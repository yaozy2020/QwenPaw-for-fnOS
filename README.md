# QwenPaw for fnOS

[![Version](https://img.shields.io/badge/version-1.1.1-1-blue)](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-green)](LICENSE)
[![fnOS](https://img.shields.io/badge/fnOS-%E2%89%A5%201.0.0-orange)](https://www.fnnas.com/)

将 [QwenPaw](https://github.com/agentscope-ai/QwenPaw) 一键部署到飞牛 NAS 的**轻量外壳包（Launcher）**。

本项目不是 QwenPaw 本体，也不打包本体源码：外壳只负责部署与运维，内核按需从 PyPI 在线安装，二者**版本双轨、互不牵连**。

## 外壳特性

- **常驻 Rust 监督者** — 内核生命周期全部经监督者执行（约 1MB 单二进制）：独立进程组启动、停止三段式组清扫（TERM 宽限 → KILL → 后代递归清扫，浏览器/MCP 子进程无孤儿）、外壳升级重启后自动收养存量内核
- **单页运维控制台** — Preact + esbuild 编译为单文件（约 54KB，brotli 传输约 15KB）：服务启停、状态卡（PID/运行时长到秒/CPU/内存）、运行日志分页回看（2000 行窗口）、版本管理、下载源选择、主题切换
- **内核版本管理** — 控制台自选版本升级/降级（语义化排序 + 降级风险提示）；独立暂存环境安装，失败自动恢复原内核并拉起服务，重复点击即时拒绝；升级指令后台执行，App 切后台/关闭页面不影响进度
- **内核不预装** — 首次安装只装外壳，打开控制台自选版本安装（Stable/Beta/历史版本），装什么由你决定
- **版本双轨制** — 外壳（fpk）升级**不改变**已安装的内核版本；内核版本由"版本钉"记录，运行环境重建时精确恢复
- **下载源运行时可换** — 安装向导选定初始源（官方/清华/阿里/中科大），控制台"下载源"选择器随时切换（另含腾讯云），落盘服务端全设备共享
- **主题跟随系统** — 默认按系统深浅色自动切换，可手动固定
- **端口自定义** — 默认 `18088`，向导与控制台均可修改；旧默认端口 `19091` 冲突时自动升迁自愈
- **预压缩传输** — 静态资源按 `Accept-Encoding` 协商发送 brotli/gzip

关于 QwenPaw 本体：多通道对话（微信、QQ、钉钉、飞书、Discord、Telegram 等）、多智能体协作、定时任务、Skills 无限扩展，支持 Ollama 等本地大模型完全离线运行，数据全部留在本地。由 [AgentScope](https://github.com/agentscope-ai) 团队开发。

## 安装引导

### 前置要求

- 飞牛 NAS 系统 **≥ 1.0.0**
- 依赖应用 **nodejs_v24** 与 **python312** 由系统在安装时自动拉起，无需手动安装
- 如需使用 Ollama 本地模型，请先在飞牛应用商店安装 **Ollama** 应用

### 安装步骤

1. 从 [Releases](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases) 下载最新 `.fpk` 安装包
2. 飞牛应用商店 →「手动安装」→ 上传 `.fpk` 文件
3. 按向导完成三步配置：

| 向导步骤 | 配置项 | 说明 |
|:--|:--|:--|
| 安装源设置 | PyPI 安装源 | 内核从该镜像在线下载，国内推荐**清华源**（也用于运行环境损坏时的重建恢复） |
| Web 端口设置 | 服务端口 | 默认 `18088`，可改；填 `19091` 会自动迁到 `18088` |
| 日志级别 | info/debug/warning/error | 常规使用选 `info` |

4. 等待安装完成——外壳安装很快（仅控制台与脚本，**不含内核**）

5. **首次安装内核**：打开飞牛桌面的「QwenPaw 控制台」→「版本更新」→ 选择版本（首次建议 **Stable 稳定版**）→ 点击安装。内核 wheel 约 40MB，从所选镜像下载，需数分钟

6. 安装完成后，QwenPaw 内核在服务端口（默认 **18088**）启动；飞牛桌面会出现**两个图标**：QwenPaw（本体 Web 端）与 QwenPaw 控制台（运维面板）。

## 升级与版本管理

| 操作 | 入口 | 对内核的影响 |
|:--|:--|:--|
| **外壳升级**（控制台/脚本/图标优化等） | 应用商店手动安装新版 `.fpk` | **无**——内核版本保持不变（版本双轨制） |
| **内核升级/降级** | 控制台「版本管理」 | 显式选择目标版本，进度与 pip 日志实时可见；pip 失败自动尝试拉起现有内核，安装进行中重复点击即时拒绝 |

> 内核降级有风险提示：旧版本可能无法识别新版本的配置字段与记忆结构，请谨慎操作。

## 卸载

卸载向导可选**保留应用数据**（重装后无缝恢复记忆与配置，推荐）或**彻底删除**（清空所有记忆、工作区与运行时环境）。

## 项目结构

```
QwenPaw-for-fnOS/
├── app/
│   ├── bin/                  # 监督者二进制（构建时放入）
│   ├── ui/                   # 迁移期原生 CGI 入口（index.cgi 静态服务、api.cgi 兼容桥）与桌面图标
│   └── www/                  # 控制台编译产物（由 web/ 构建，不入库）
├── cmd/                      # 薄生命周期入口，转发给 Rust 固定子命令
├── config/                   # 权限与资源配置
├── supervisor/               # Rust 监督者、管理核心、HTTP/私有控制通道
├── web/                      # 控制台前端源码（Preact + TypeScript，esbuild 出单文件产物）
├── wizard/                   # 安装/升级/卸载/配置向导
├── scripts/                  # lint、事务/边界回归、preflight、版本同步等工程脚本
├── manifest                  # 应用清单（版本 SSOT）
├── build.sh                  # 构建打包（cargo 监督者 + esbuild 控制台 + fnpack）
└── ICON.PNG / ICON_256.PNG   # 应用图标
```

### 运行时职责与事务边界

主线为 **fnOS 生命周期入口 → Rust 监督者/共享管理核心 → 受管 Python venv 中的 QwenPaw**。监督者管理 HTTP 只监听 `127.0.0.1:${SUPERVISOR_PORT}`；平台回调通过权限受限的私有 `supervisor-control.sock` 提交控制操作。该私有 UDS 不是 fnOS 统一网关 Socket，也不证明网关鉴权或管理员授权已验证。

- `supervisor/src/management.rs` 提供 HTTP 与平台控制入口共用的管理核心；`supervisor/src/control.rs` 实现私有控制 UDS。`supervisor/src/api/` 按状态、生命周期、安装、配置和日志分组，状态查询不写桌面配置；`desktop.rs` 负责桌面入口的定点原子更新。
- `supervisor/src/install/` 分离安装编排、持久化环境事务与子进程执行。失败路径先停新实例，再回滚环境，最后恢复旧服务；安全收尾完成前不释放安装槽。
- `runtime_settings.rs` 与 `settings_transaction.rs` 统一端口、日志级别和桌面配置变更：持久化恢复日志后执行，内核健康检查通过才提交，中断则在启动时恢复。平台配置回调由 Rust 子命令处理，不再自行构造管理 JSON 或写配置。
- `supervisor/src/runtime.rs` 的 `RuntimeAdapter` 集中以固定 argv 调用受管 venv、pip 与 QwenPaw；这不代表 Python 内核依赖被移除，也不把管理策略交还给 Python。
- `app/ui/api.cgi` 是薄入口，转发到 Rust 原生 `web api-cgi` 兼容处理；`app/ui/index.cgi` 转发到 Rust 原生静态资源服务。两者仍随包使用，属于迁移期兼容桥，不是 fnOS 网关登录或管理员授权实现，也未宣称已退役。
- 无法完成恢复时禁止自动启动和运维变更，保留状态、日志查询及故障文件供排查。停止监督者时禁止新内核启动，并等待进行中事务安全收尾。
- `web/src/installTask.mjs` 独立判定安装任务归属；受理序号与监督者进程身份都匹配才确认终态，响应丢失时明确显示无法确认，不猜测成功。

现有本地回归入口包括 `python3 scripts/test-platform-callbacks.py`、`python3 scripts/test-native-api-cgi.py`、`python3 scripts/test-native-index-cgi.py`、`python3 scripts/test-runtime-venv-contract.py`、`python3 scripts/test-management-boundary.py`、`python3 scripts/test-supervisor-api.py`、`bash scripts/test-lifecycle.sh`、`bash scripts/test-config-callback.sh`、`bash scripts/test-venv-recovery.sh`、`STRICT_CI=1 bash scripts/lint.sh` 与 `python3 scripts/gen-audit.py --check`。`test-supervisor-api.py`、`test-venv-recovery.sh` 及 lifecycle 脚本中的真实监督者场景需要先构建 release 二进制；按各脚本要求使用隔离临时目录，不操作已部署服务。前端可用 `npm run typecheck --prefix web`、`npm test --prefix web` 和 `npm run test:e2e --prefix web`。

### 当前本地候选验证（非生产可用声明）

2026-11-04 初审整改后的 1.1.1 RC1 本地包为 `build/output/qwenpaw-1.1.1-1-x86.fpk`（2,071,727 字节，SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`）。Rust 222+13、Node 30、严格 Clippy/typecheck、lint、audit、自测、API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 与 Chromium 1 项均通过；原始 SIGKILL→service stop/restart 已在隔离环境实测通过。完整结果与边界见 [`docs/session-handoff-2026-11-04.md`](docs/session-handoff-2026-11-04.md)。

候选未经 fnOS 真机网关/UDS/鉴权、回调执行窗口、最低版本、实际平台停止预算、真实 PyPI 上游包、WebKit 或 fnOS App 验证；候选通过本地验证不表示生产可用或已发布。

## 版本历史

版本呈现自当前候选起统一为：架构变更前的原 `1.0.x` 归入 `0.0.x`，引入常驻 Rust 监督者后的原 `2.x` 归入 `1.x`。这是文档与后续版本的归一化规则；既有 Git tag 和已发布 Release 名称保持原样，不重写历史发布对象。

### v1.1.1-rc.1（当前候选，FPK 版本 1.1.1-1）

- 管理层统一迁入 Rust/native CGI 薄桥，生命周期、配置、安装和恢复共用管理核心与私有控制 UDS；
- 修复崩溃后 service 恢复、提交写失败、Web 停止/卸载门禁和跨实例任务归属；
- 前端类型纪律、真实 venv、并发与故障恢复回归纳入发布门禁。

### v1.1.0-rc.1（历史候选，FPK 版本 1.1.0-7）

- **控制面板运维闭环**: 支持仅卸载内核并保留用户数据，卸载后可从空 venv 重新安装；同版本显示重新安装；镜像故障持续提示换源与网络检查；明确飞牛控制台和 QwenPaw 独立管理员两套鉴权；桌面入口维护 `noDisplay` 状态，但当前 fnOS 实测不会在运行时动态隐藏已注册图标；
- **事务式内核安装**: pip 只修改独立暂存 venv；启动、精确版本健康检查或版本钉写入失败时恢复原环境和版本钉，成功后才清理旧环境备份。安装期间需预留一份当前 venv 的临时磁盘空间；
- **版本查询同源**: 可用版本列表改为读取当前下载源的 PEP 503 Simple Index，与 pip 实际安装源一致，不再固定查询官方 PyPI JSON；
- **状态热路径优化**: 内核版本与认证状态短缓存，安装终态主动失效版本缓存；状态快照和镜像查询移出 Tokio 工作线程；
- **可复现构建**: 以 `SOURCE_DATE_EPOCH` 归一化 FPK 外层与 `app.tgz` 的文件顺序、owner 和 mtime，连续构建可逐字节比较；
- **仓库治理**: 删除已退出构建链的 Vue 历史 UI、上游源码同步脚本与旧静态资源，Preact + fpkstudio 为唯一活跃链路；Rust 21 项回归测试通过。

### v1.0.1（当前稳定版）

- **生命周期真实性**: venv 创建、激活和 pip 升级失败立即非零退出，并通过 `TRIM_TEMP_LOGFILE` 向用户报告，不再出现失败假成功；
- **端口配置闭环**: 控制台修改端口后同步更新飞牛桌面入口，失败同时回滚运行配置与入口；安装及设置路径统一拒绝保留端口，同端口请求可自愈历史错配；
- **并发运维防护**: 启停、重启、安装、改端口、重置认证和版本钉修改统一互斥，安装期间及并发变更立即返回 409，避免操作半修改的运行环境；
- **升级恢复与协议一致性**: 外壳升级完成后显式恢复监督者与内核服务；CGI 透传监督者 HTTP 状态码，安装冲突真实返回 409；
- **质量验证**: Rust 14 项测试覆盖动态端口、精确版本、UTF-8、端口入口同步和并发互斥，完整生命周期与 FPK 审计通过。

### v1.0.0-rc.1（FPK 版本 1.0.0-1）

- **发布候选安装状态机**: 新增 `Starting` 非终态，健康检查与版本钉写入成功后才进入 `Done`；健康检查使用实际配置端口并精确匹配目标版本，启动失败、健康超时和版本钉失败均如实上报；
- **日志与质量门禁**: 运行日志和 pip 日志共用 UTF-8 安全截断，覆盖中文与 emoji 边界；Rust 12 项回归测试与 Clippy 严格检查通过；
- **构建系统重构升级**: 打包流水线改用 `shuangji66/fpkstudio`（Unix Socket 管道架构），实现强沙箱隔离物化与 100% 运行时纯净白名单审计，杜绝开发机任何未跟踪文件混入；
- **桌面入口与图标闭环**: 补全桌面必需的 4 项图标（`console_64/256` 与 `icon_64/256`），主对话入口切换为独立标签页直连；
- **安全与 CORS 治理**: 彻底收紧跨域通配符，杜绝任意 Origin 反射与 Credentials 组合风险；
- **管理员真实状态感知**: 移除前端静态硬编码谎报，接入 `/api/auth/status` 实时四态感知（未初始化管理员醒目安全警示、已启用密码保护、免密模式、状态未知）；
- **数据销毁零残留**: 彻底删除模式补齐对应用密钥与认证目录（`.qwenpaw.secret/auth.json`）的严格清理，生命周期门禁增加强断言保护；
- **端到端安装就绪健康门禁**: pip 安装成功后由监督者轮询确认 WebUI 与目标版本真实响应，彻底消除假就绪。

### v1.0.0-15

- **Double-Submit Cookie CSRF 防护**: 彻底解耦网关反代与网络拓扑，彻底杜绝 Host 头重写误杀 403；
- **SemVer 分级比对与降级保护**: 修复版本列表降级倒转逻辑，消除初装预发布幽灵版本干扰；
- **默认阿里云源**: 提升云端与机房 IP 场景下的 pip 下载稳定性。

### v1.0.0-12

- **安全(P0)**:内核安装的版本参数不再拼进 `bash -c` —— 改为渠道枚举(latest/beta/精确版本)+ PEP 440 字符白名单 + argv 直传 pip,消除 Shell 命令注入;版本钉入口同一白名单
- **安全(P0)**:桥接 api.cgi 增加方法白名单(读=GET/变更=POST,违规 405)与 Origin/Referer 同源校验(403),未知 action 404;lint 新增 405/403/404 回归用例
- **安全(P0)**:supervisor-token 全生命周期 0600(新建即 0600、启动收敛、写入后复核);随机源不可用拒绝启动,不再静默生成弱 Token
- **安全(P0)**:收回无用权限 —— 不再加入 docker 组;删除零调用的 trim.file.* API scope
- **稳定性**:改端口与重置认证改事务式 —— 失败回滚旧端口/恢复认证备份并以健康检查(进程存活+端口可达)为准,不再假报成功;`spawn().unwrap_or(0)` 清除
- **稳定性**:版本钉仅在 pip 成功且 venv 元数据版本核实一致后原子写入(0600);pip 失败/元数据不符绝不污染可用版本钉
- **稳定性**:SIGTERM 先取消安装(整组回收 pip)再停内核;监督者重启凭 install.pid 清理孤儿 pip;生命周期脚本统一 supervisor.pid 口径(旧 qwenpaw.pid 死引用清除)
- **发布完整性**:git 执行位修复(CGI/生命周期脚本/构建脚本 100755);build.sh 改 staging 白名单(supervisor/target 不再入包)+ 构建后自动审计(audit-fpk.sh);cargo 构建失败终止出包且先删陈旧产物
- **CI**:新增 rust job(cargo test/clippy -D warnings/build --locked);git 执行位检查;action 固定提交 SHA;npm ci 锁文件一致
- **治理**:/logs 与 pip 日志尾部窗口读取;启动时大日志单代轮转;docs/backend-design.md 标注为历史文档
- **P1 生命周期审计整改**:监督者停止三处入口(main/lib.sh/卸载回调)统一为 `supervisor_ctl.sh` 身份核对安全停止 —— PID 文件只是提示,发信号前核对 `/proc/<pid>/exe` + starttime 戳,PID 复用/陈旧 PID 绝不误杀无关进程(可动态复现的缺陷);外部停止宽限 15s→30s(原值小于监督者内部最坏清理 ~16.5s,会在后代清扫完成前强杀,留下孤儿);取消安装新增 Cancelled 终态(注入式测试复现"运行态已建立、PID 未登记"窗口,取消绝不拉起 pip、不写版本钉、不触发自愈重启);改端口/重置认证的回滚闭环 —— 先停止失败的新实例再恢复配置,恢复本身也要过健康检查,写回失败如实上报
- **移动端安装闭环**:安装触发改为可读响应的 POST(受理 seq 确认接单,sendBeacon 仅作切后台兜底);busy 响应如实展示并自动转入进度查看;弹窗按任务 seq 跟踪,修复"首拍已终态被误判为未送达"的漏判;重开控制台/换设备时检测到进行中任务直接附加查看,不再重复创建;pip 看门狗(无输出 15 分钟/总时长 60 分钟)整组回收,安装不再可能永久锁死 busy 槽

### v1.0.0-11

- 第二轮独立审计整改：安装失败自愈在裸线程直调 tokio spawn 必 panic 并毒化日志锁 → kernel spawn 改 `std::process::Command`（任意线程安全，收尾线程回收僵尸并落退出日志）
- 升级日志渲染误加 `.catch` 于 void 调用（历史版本链路死亡）→ 删除；install 终态双 emit 修复
- supervisor-token 每次启动收紧 0600（存量宽松权限文件一并治理）
- 故障注入验证：沙箱真实触发 pip 失败 → 零 panic、内核自动拉起、二次安装正常受理

### v1.0.0-10

- 首轮独立审计整改：版本管理同名遮蔽自递归（blocker，历史版本/选版本全链路失效）；pip 失败不再留停机态（自动拉起现有内核）；pip 日志 UTF-8 双侧加固；升级日志"失败"关键词；过渡态升级弹窗守卫

### v1.0.0-9

- 控制台全量迁移 Preact（插值自动转义，escapeHtml 结构性退场）
- 升级卡顿修复：忙检查前移（安装中重复点击不再误停内核）、监督者新增 install-status/install-log 轻端点（轮询热路径去掉 python3 冷启动）、轮询节流守卫

### v1.0.0-8

- Svelte 打样（StatusCard 组件 + store 契约）；同组件实测 Svelte 5/4/Solid/Preact 尺寸与工具链后，选定 Preact 并于 -9 全量迁移

### v1.0.0-7

- 生命周期实测整改：停止级联漏 await（假成功）修复、进程后代递归清扫（停止/重启零孤儿）、iOS 切后台升级触发改 sendBeacon、下载源完整 URL 直通监督者

### v1.0.0-6

- 前端编译管线（esbuild 单文件产物，编译不过即出不了门）；运行日志分页回看（每页 100 行、2000 行窗口）；下载源自定义下拉

### v1.0.0-5

- 控制台"下载源"选择器（落盘服务端全设备共享，升级时生效）；主题默认跟随系统；服务状态卡补 CPU 占用与内存占用、运行时长精确到秒

### v1.0.0-4

- 独立审计整改：升级反馈链路闭环（pip 日志 JSON 转义/文件名统一/phase 完成判定）、版本语义化排序（2.10.0 > 2.9.9）、版本列表失败如实降级

### v1.0.0-3

- 修复桥接脚本输出非法 JSON（bash 参数展开花括号陷阱）导致的控制台动态渲染全静默失效

### v1.0.0-2

- 监督者二进制安装路径修正

- **架构升级:常驻 Rust 监督者接管内核生命周期**——安装/重启/改端口全部经监督者执行,App 切后台、网关回收不再影响任何后台作业(App 切后台杀死安装的事故从架构上消失)
- api.cgi 退役为瘦桥(token+转发),cmd/main 只管监督者进程;内核 18088 直连不变
- SSE 实时状态推送(预留,控制台轮询兜底)
- 监督者依赖 wheels 随包分发,离线安装

### v0.0.10

- **进程管理保底机制**：内核以 setsid 独立进程组启动，停止时组信号连带浏览器/MCP 等子进程（TERM 宽限 10s 后组 KILL 清扫，实测 TERM 免疫子进程可被清除）
- 安全阀：组信号仅在确认 setsid 启动时启用，旧版启动的进程自动降级为单进程信号，不误伤平台进程

### v0.0.9

- 修复进程管理顽疾：pid 文件记录的是 bash 包装进程而非内核本体，停止/重启从未真正结束内核（孤儿进程占端口）；启动改 exec 形式，pid 即内核进程

### v0.0.8

- 脚本路径全面接入 TRIM_* 变量（带回退，对齐官方规范）
- 安装与升级失败信息接入 TRIM_TEMP_LOGFILE 官方用户可见通道

### v0.0.7

- **首次安装不再预装内核**：外壳安装即完成，用户打开控制台「版本更新」自选版本安装（对齐 DSH 的 not-installed 模式）
- 已有环境逻辑不变：venv 健在仅修复不动内核，环境被清按版本钉恢复
- 控制台新增「内核未安装」状态引导；启动/改端口在内核缺失时给出明确提示
- 安装向导移除「首发安装通道」（通道选择由控制台安装时自行决定）

### v0.0.6

- 修复修改端口后内核启动崩溃的隐患（change_port 重启路径补齐依赖托管环境变量）
- 新增 lint 规范化门禁（bash -n / shellcheck / 内嵌启动脚本展开验证）并接入 CI

### v0.0.5

- 修复控制台降级后版本钉写入格式错误、导致下次外壳升级失败的问题（写入器与读取器双侧加固）

### v0.0.4

- 取消旧版数据自动迁移逻辑及相关文案
- 修复内核 2.3.0b2+ 在 fnOS 的启动崩溃（注入 `QWENPAW_DEPENDENCY_STATE_DIR`）
- 版本钉全路径闭环：外壳升级、环境重建均不改变内核版本

### v0.0.3

- 应用说明、README 安装引导、升级向导文案对齐实际功能
- 明确升级向导中 PyPI 镜像仅在运行环境重建时使用

### v0.0.2

- 修复外壳升级隐式变更内核版本的问题（v0.0.1 装包后内核被自动升级至 2.3.0b2、因上游启动锁 bug 失败的根因修复）
- 新增内核版本钉，`upgrade_callback` 遵守版本双轨制

### v0.0.1

- 控制台性能优化：预压缩协商传输（brotli/gzip）、轮询后台暂停
- 图标扁平化（去内层蓝盘，对齐 DSH 风格）并量化，包体 246K → 143K
- CI/Release 工作流对齐零构建架构

### v0.0.0

- 独立发版：确立外壳/内核版本双轨制
- DSH 风格极简控制台（双模态、版本自选、降级保护）
- 自定义端口支持

## 常见问题

### 控制台显示「内核未安装」？

首次安装只部署外壳，内核需要手动安装：控制台「版本更新」→ 选择版本（建议 Stable）→ 点击安装，从 PyPI 镜像下载 wheel（约 40MB）及依赖，视网络情况需数分钟。安装完成后服务自动启动。

### 外壳升级会把 QwenPaw 内核也升级吗？

不会。外壳与内核版本双轨，外壳升级只更新控制台与生命周期脚本，内核版本原样保留（由版本钉保证）。内核升级请走控制台「版本管理」。

### 端口 19091 被占用或想换端口？

向导与控制台均可修改服务端口；安装时填入 `19091` 会自动改用 `18088`（冲突自愈）。运行中修改端口请在控制台「端口设置」操作。

### 使用 Ollama 时 API Key 怎么填？

连接 Ollama 时 API Key 字段不能为空，填写任意非空字符串（如 `ollama`）即可。

### 点击「安装内核」或修改设置提示来源/CSRF 校验失败？

这是由于网关反代重写 Host 头或页面 Token 未同步导致的：
1. 请确保外壳已更新至 **≥ v1.0.0-14**（该版本采用 Double-Submit Cookie 机制，完全解耦网络拓扑与反向代理环境）。
2. 在浏览器中按 **`Ctrl + Shift + R`** 强制刷新控制台页面，重新同步会话凭证后再行操作。

### 内核安装失败或提示「Could not find a version ... / from versions: none」？

这通常是网络环境访问当前镜像源异常引起的（例如部分高校开源镜像站对公有云机房 IP 或特定客户端存在频控/403 拦截）：
- 打开控制台「版本更新」面板，将「下载源」切换为**「阿里云镜像源」**或官方 PyPI 源后重新尝试安装。

### 手机端无法在应用中心手动上传 `.fpk` 文件？

部分移动端浏览器对系统文件选取控件支持受限。推荐使用 PC 桌面浏览器；若在手机端操作，可先将 `.fpk` 文件放入 NAS 本地目录，再在应用中心手动安装弹窗中选择**「从 NAS 添加」**直接选取文件安装。

## 交流与反馈

- **GitHub Issues**：[提交反馈](https://github.com/yaozy2020/QwenPaw-for-fnOS/issues)

## 相关链接

| 项目 | 地址 |
|:--|:--|
| QwenPaw 本体 | [agentscope-ai/QwenPaw](https://github.com/agentscope-ai/QwenPaw) |
| AgentScope 团队 | [agentscope-ai](https://github.com/agentscope-ai) |
| 飞牛 NAS | [fnnas.com](https://www.fnnas.com/) |

## 许可证

本项目基于 [Apache-2.0](LICENSE) 许可证开源；QwenPaw 原项目同样采用 Apache-2.0 许可证。
