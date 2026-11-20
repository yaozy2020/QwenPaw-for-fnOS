# 管理层现状契约盘点

- 状态：按 1.4.7-1 候选工作树更新；本文记录源码契约，不能替代包内回归或飞牛真机验收。
- 范围：fnOS 生命周期薄入口、Rust supervisor/管理核心、Gateway、私有控制 UDS、受管运行时及 Web 客户端。
- 当前外壳候选为 `1.4.7-1`。目标版本列表在面板内展开，选中、取消、键盘浏览都不发起安装或二次查源；下载源、降级确认和备份继续使用原流程。保留数据恢复安装时，仅当实际 var 根属主/属组正确且平台权限恰为 0770，才把该根收敛为 0750；不递归、不修改属主，其他权限与升级检查保持 fail closed。运行时长满 24 小时仅显示到分钟，不改变采样。概览在现有 SSE 上按需订阅 `watch=metrics`，每秒只发送 PID、启动时刻、CPU 和 RSS；安装/登录状态与控制操作仍取完整快照。已安装但版本未确认不视作首装。配置、授权与备份控件按首次访问挂载，此后保留；历史导航不可用时两个分区均加载。未访问授权分区时也要处理已完成的授权回调。Vue 管理面板与 DSH 的逐项对照见下文。`1.3.x` 的性能专项及其历史证据见 `performance-governance-1.3.md`。manifest 按官方 JS SDK 要求声明 `micro_app=true`。
- 当前管理 Web 架构为 Gateway-only。G4 已实施：生产 CGI 文件、CGI 子命令、前端 action fallback、CGI 专属测试和 CI 门禁均已删除。下文按当前实现描述，不把历史 CGI 路径写作现状。

## 入口与共享管理核心

| 入口 | 当前契约 |
|---|---|
| `cmd/main` | 固定转发 `service start|stop|restart|status` 到 `qwenpaw-supervisor`；定位与 `exec` 是入口职责，服务身份校验、锁、就绪和停止策略由 Rust 实现。 |
| `cmd/*_callback` | install、upgrade、configure、uninstall 等 fnOS 回调转发固定 `platform` 子命令；向导环境变量由 Rust 校验与处理。平台回调时序/重试语义须按平台证据描述。 |
| `supervisor/src/main.rs`、`cli.rs` | 启动装配共享 `ManagementCore`；提供常驻 `serve`、`service`、`platform`、`doctor --json` 等原生子命令。生产 CGI 子命令 `web api-cgi` / `web index-cgi` 已删除。 |
| `supervisor/src/management.rs` | HTTP、Gateway 与私有平台控制通道共用管理核心，集中操作互斥、恢复门禁、安装提交、运行时健康与初始化等管理规则。 |
| `supervisor/src/platform.rs`、`service.rs` | 分别承接平台生命周期/回调与服务管理；Shell 入口不保留安装、配置、停止或删除策略。服务身份仍先核对 PID 与 starttime；`/proc/<pid>/exe` 权限受限时回退读取完整 `cmdline` 可执行路径，两侧路径规范化后比较，读取失败或身份不匹配仍拒绝。一致 supervisor 直接复用，路径变化才停止；orphan 接管需历史安装证据且仅接受唯一当前目标，健康内核不因重装被停止，多实例或身份不明失败关闭。 |

## Gateway、HTTP 与 UDS 边界

- fnOS `qwenpaw.console` 配置 `/app/qwenpaw/` 管理入口，`qwenpaw.main` 配置 `/app/qwenpaw/web/` 对话入口，两者共用 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock` 和默认 fnOS iframe 会话链。控制台的“进入 Web 对话”在 PC 浏览器打开新标签页，在 fnOS App 内打开全屏 iframe，返回先关闭覆盖层；不使用 CGI fallback，不裸连 QwenPaw 内核端口。
- Gateway UDS 与 `${var_dir}/supervisor-control.sock` 私有控制 UDS 分离。私有 control UDS mode 为 0600，通过 `SO_PEERCRED` 核验 peer UID，只供受信的平台私有控制操作；它不是浏览器或 fnOS Gateway 入口。
- 管理 HTTP loopback 与 Gateway 是不同传输入口，共用管理核心。不能因本机 loopback 或私有 control UDS 请求携带 `X-Trim-*` 就当作 Gateway 身份。
- Rust `web_proxy` 将 `/app/qwenpaw/web/` 下的请求代理到上游 QwenPaw Web，保留上游路径，支持流式 SSE；未迁移时传递上游所需的 Bearer Authorization，迁移后不向内核传递浏览器的 Bearer；管理系统自身的 CSRF 校验与上游请求处理相互分离。WebSocket 明确返回 HTTP 501，不能宣称已支持 WebSocket。
- Gateway 总连接上限为 128；达到上限后为新连接等待最多 2 秒，有连接结束则继续服务，仍满员才返回 HTTP 503 与 `Retry-After: 1`。SSE 独立配额保持为 8。
- **原生密码恢复（1.4.6）**：`POST /kernel/admin-password` 只接收新密码，沿用管理令牌、Gateway 管理员/CSRF 及操作互斥。内核通过现有的双方进程校验 UDS 确认已有账号和可选桌面绑定，使用原生哈希及签名密钥加密，原子替换凭据；不注销飞牛会话或解除旧反代保护。缺账号、不兼容或状态无法确认时不回退至清空注册。旧 `/kernel/reset-auth` 不再注册，历史重置 journal 仅启动恢复读取。
- **1.4.3 当前契约**：不再注册 `POST /web-proxy/password`。新装使用原生账号。保留旧密码记录及原登录流程直至管理员经管理写边界 `POST /web-proxy/use-native-login` 明确确认，并经双方进程身份验证的私有通道通过原生登录及账号归属校验（已绑定时也对照绑定身份）。失败不删除记录；成功后清除旧会话，不改原账号和飞牛绑定。外部 TCP 不提供管理路由，不以本机、反代或旧 cookie 代替原账号。受管内核由 supervisor 显式开启认证；不兼容的内核不能用于切换。
- 以下为早期版本契约；与上条不一致时以上条为准。1.4 开发时：独立 TCP Web 反代（含 loopback）在转发页面和 API 前验证当前实例的反代会话。可信局域网 HTTP 和外部终止 TLS 的 HTTPS 共用此密码。登录提交必须带与 Host（含端口）一致的浏览器 Origin；HTTPS 保留 Secure 和 __Host- 会话 Cookie，HTTP 会话不使用这两个属性；两者都不转发给内核。默认实例由监督者指定，不接受请求指定实例；键不含端口。管理写操作 `POST /web-proxy/password` 沿用既有令牌和 Gateway 管理员/CSRF 边界，状态只报告是否已配置。飞牛 Gateway 不走这个密码门禁。未迁移实例保留受管内核原有认证，已迁移实例只接受下述两个入口。
- 旧内核的会话校验不是 HTTP 健康探针。`2.3.0` 与 `2.3.0.post1` 的标准版 `/api/auth/verify` 正常返回 `200 {"valid":true,"username":"…"}`；关闭认证时也可能返回 200，但用户名为空。需要上游会话的反代路径仅在 200、JSON 结构正确、`valid` 为 true 且用户名非空白时转发。响应异常、没有主体或已撤销均不放行；此判定不把旧会话升级成反代会话。
- **1.4.0-4 当前例外（取代此前“迁移后关闭内核 TCP/无密码拒绝访问”的约定）**：本机服务端口保持回环绑定，根路径进入原生 Console，`/api/` 由内核认证。独立反代默认无额外密码，未设或明确移除时使用原生认证，不复用迁移 owner 或本机白名单。已有密码继续生效；密码记录损坏时不放行。匹配适配的私有传输区分已核验身份和普通访客。不支持新适配的旧内核仍保留敏感 API 会话检查。飞牛入口不变。
- **1.4.0-1 迁移契约**：`POST /entry-login/migrate` 同时要求管理调用权限和内核核验的原账号会话。归属确认后才原子保存 `entry-login-default.v1`（0600，版本、实例、用户名；不存密码或令牌），不修改旧账号文件。新请求只经核验双方进程身份的 Linux 私有 UDS；平台已验证的管理员或已登录独立反代共享该单实例的授权，浏览器不取得内核管理员令牌。已迁移的内核 TCP 不承载业务请求；无效记录、未安装适配或失去原父进程时拒绝，不回退到旧登录。若仅 supervisor 意外退出、原内核存活，须在飞牛管理台重启受管内核以重建私有通道。
- **移除旧鉴权是后续版本，不属于迁移版**：对保留数据的升级与重装，先核验完整迁移记录，再停服务或改数据；缺失/损坏不能当作新装或自动分配管理员。未迁移用户先运行过渡版；真正新装才使用新初始化流程。不能简单删除旧账号文件或永久保留任一旧令牌作为万能凭证。
- 管理端鉴权状态探针复用内核 base-path：原生补丁启用时请求 `/app/qwenpaw/web/api/auth/status`，否则请求 `/api/auth/status`；不得用根路径探测已挂载的内核。
- 生产 CGI 文件、CGI 子命令、前端旧 action fallback、CGI 专属测试与 CI 门禁已删除；历史 CGI 迁移记录不属于当前生产契约。

## 配置、安装与运行时

- 配置入口统一进入 Rust 平台模块及 `ManagementCore`。`settings_transaction.rs` 对端口、日志级别及桌面入口维护可恢复事务；状态查询不应改写配置。镜像由 Rust 控制面持久化。
- 安装编排和持久化环境事务位于 `supervisor/src/install/`。安装使用独立 staging/rollback venv；健康检查、版本校验及提交决定遵循事务状态；未提交失败先停止新实例、恢复环境与 pin，再恢复旧服务。恢复不完整时保留证据并阻断写操作。
- `supervisor/src/runtime.rs` 的 `RuntimeAdapter` 通过固定 argv 调用受管 Python/venv、pip、QwenPaw CLI，以及限定的 metadata/import 探针和 init。管理策略、JSON 业务、联网版本解析不由 Python 实现；QwenPaw 自身的 Python/Node 依赖仍是受管运行时要求。
- `VenvTransaction::activate` 在激活暂存环境前，由 `supervisor/src/install/io.rs` 的 `relocate_console_script_shebangs` 将匹配的 console-script shebang 调整为活动 venv 路径。`scripts/test-runtime-venv-contract.py` 提供真实 venv 的 `sys.prefix`、import、shebang/执行及后续 pip 契约检查入口。
- `supervisor/src/version_index.rs` 使用 Rust HTTP 与 HTML 解析实现 Simple Index 版本查询；TLS、网络可用性与 fnOS 实际镜像访问仍须按相应测试/平台环境验证。
- `cmd/lib.sh` 与 `cmd/supervisor_ctl.sh` 已删除；`cmd/main` 和平台回调为固定 Rust 子命令的薄转发入口。

## Web 客户端及任务协议

- 前端使用同源 Gateway REST/API 路径，不再基于页面地址推导 `api.cgi`，也没有 action 协议回退分支。
- 1.3 候选使用 Preact；1.4 开发工作树已用预编译 Vue 3 单文件组件替代 Preact，不并行加载两套框架。`authorizedDirsPanel.ts` 独立管理目录绑定、重新授权、解绑及原生打开；manifest 按官方要求声明 `micro_app=true`，使用 `@trimjs/web-app 0.5.0` 的 `TrimApp.ready()` 与 `openFileManager(path)`，桌面 Penpal 与手机 Flutter 由 SDK 统一适配；失败只追加 getAppMessage 是否为空、字段名和 appApi 等非敏感诊断，独立普通浏览器隐藏入口；面板显示真实返回，异常、false、success/ok=false 或非零 code 明确报错，null/undefined 不伪报已打开；`logPanel.ts` 独立管理单窗口日志合并、竞态、滚动、导出与清空；`settingsPanel.ts` 独立管理服务端口、Web 反代、镜像与认证；`versionPanel.ts` 独立管理版本查询、选择、比较与安装动作状态；`installCoordinator.ts` 负责安装确认和受理；`installProgressPanel.ts` 独占安装窗口、轮询、runId 与任务持久化；`statusPanel.ts` 负责状态竞态、SSE 快照和概览渲染；`installTask.mjs` 的 seq+instance 归属不变；`modal.ts` 和 `apiResult.ts` 共享弹窗与 API 结果类型；`pollingLifecycle.ts` 统一轮询的幂等启动、后台暂停、返回立即刷新与销毁清理；`navigation.ts` 独立管理移动导航、历史和监听器；`main.ts` 只负责页面编排，后续新交互应落在独立面板而非继续扩大主控制器。
- 桌面侧栏和手机固定底栏都切换 `overview`、`configuration`、`authorization`、`logs` 四个分区，手机底部预留安全区与导航空间。`#version` 是概览下的部署版本视图；无可用分区导航时降级为完整单页。健康概览不预取版本、镜像、反代或授权列表；首次安装和已有待保存授权不因延迟加载而失效。“更新内容”入口、弹窗和请求已移除。状态以 SSE 为主、30 秒兜底，运行时长在本地每秒更新且不新增状态请求。日志取消翻页，`1.4.0-2` 起桌面和手机概览均不展示日志，也不启动日志定时器；配置、版本和授权分区不读取运行日志；首次打开日志分区读取 2000 行回看窗口，后续每 15 秒读取最新 100 行并按重叠合并，后台暂停；移动端日志终端填满剩余视口，支持自动滚底、上翻暂停、导出和清空。分区切换不再强制滚动，使用 160ms 淡入并支持减少动态效果。`config/resource` 声明 `trim.file.sharedAccess/userAccess/userAcl/path` 四个授权 scope。授权目录以 QwenPaw `project_dirs` 为登记来源，选择后由上游校验并直接保存，不另设权限门禁；PUT 只提交归一化 path 与非空 label，不得回传 GET 中的 `exists/nested_with/is_workspace` 等只读字段；失败固定显示在目录区域。Rust `POST /authorized-dirs/check` 仅用于重新授权后的独立权限复查。授权监听在导航前注册，启动时读取已落盘结果，回调地址使用同源绝对 URL；页面重建后可恢复 5 分钟内的落盘授权结果，继续 PUT 保存、显示目录并在成功后清除；面板显示授权、读取、保存、完成或错误阶段；回调页检查结果发布 HTTP 响应，失败时显示状态并停止自动关闭。嵌入回调由父页面收到结果后关闭，不刷新顶层控制台；原始 query 与仅含 `result.path` 的结构化结果均可进入绑定，state 校验和路径归一化保持不变；独立弹窗保留返回兜底。解绑不删除目录或文件。
- 备份入口复用上游 `/app/qwenpaw/web/backups`，不新增第二套备份后端。当前 `1.3.3-1` 以官方 `2.3.0.post1` 为基线，只携带一套完整预构建 Console 和对应后端文件；`ConsoleStaticFiles` 覆盖预压缩协商、条件请求、HTML 禁缓存与哈希资源长期缓存。动态预加载并发仍为 16，登录页不初始化聊天插件。清单声明官方与已核验的历史哈希，未知本地修改拒绝覆盖；旧 Console 资源仅在安装事务暂存环境中按显式 `delete_files` 核验清理，不对现用 venv 或未知文件做推测删除。健康 2.3.0 在该外壳升级时经同一事务迁移到 post1，失败回滚且保留用户数据。完整载荷直接复制，不再下载其他版本源码或运行 npm/Vite。
- 日志后端 `/logs` 与 `/kernel/install-log` 共用 Rust 尾读器：以请求开始取得的文件长度为上界，从末尾按 8 KiB 分块回读，足够所需行数即停止，最多读取 256 KiB；仅为最终返回行执行 UTF-8 安全截断（2000 字节上限，超长附省略号）。窗口首个残行丢弃、CRLF、无末尾换行和非法 UTF-8 替换语义保持不变；读取中新增内容留待下次刷新，文件缩短导致读取失败时仍按原有约定返回空列表。默认/上限仍分别为运行日志 80/2000 行、安装日志 40/500 行，安装任务身份和鉴权规则不变。
- 合法 `seq + instance` 任务身份存入 `sessionStorage`。页面重建及 `pageshow` 恢复原任务的轮询，不重复 POST 创建任务；存储不可用时安全降级。不得在无法确认任务时猜测成功或重复提交。
- 语言只在首次初始化时默认中文，不覆盖已有英文。

## 1.4.1 与 DSH 的功能对照

参考 [shuangji66/dsh 的管理台功能](https://github.com/shuangji66/dsh#功能特性)。对标常用管理任务，不把 DSH 的进程模型作为 QwenPaw 的运行契约。

| 管理任务 | QwenPaw 入口与差异 |
|---|---|
| 查看状态、资源占用、启停、重启与进入 Web | 概览；桌面经同源网关开新标签，飞牛 App 内打开可返回的全屏视图。没有插件或厂商账号入口。 |
| 查询版本、换源、安装与查看进度 | 概览 → 部署版本。保留已验证版本优先、手动检查、后台任务重新附加；源失败显示旧列表但禁止安装。安装结束重新读取状态，已切换内核版本时不再采用旧列表针对上一个版本给出的「升级／重装」判断。从 1.4.2 起首页始终显示单一带标签的版本选择框；按最早制品发布时间倒序（无日期放末尾，同日时刻相同或无日期按 PEP 440 降序），升级/降级不看列表位置。多版本共存和后台自动查源明确不做；下载取消未移植。 |
| 本机端口与外部反代 | 配置；保留本机原生服务与飞牛网关的边界，独立反代仅兼容升级前已设的密码。浏览器进入 Web 首页而内核未就绪时，返回带「重新检查」的轻量 503 页面；不仿造内核启动阶段，也不在停机时反复轮询。API 不返回这个 HTML。 |
| 工作目录与备份 | 授权与备份；查看、绑定、重授权、解绑，按宿主能力打开目录。日常备份使用 QwenPaw 自身界面；面板安装请求可选 `backup:true` 在已停机的安装 worker 中先创建并验证私有 `preinstall-data.tar`，不新增下载端点、不自动回写用户数据。「授权与备份」始终可查看离线恢复步骤。 |
| 日志 | 日志分区查看、追尾、导出与确认清空；同一受限 SSE 仅给主动订阅的页面发送文件变化通知，日志文本仍由原来的有界尾读接口提供。后台和非日志分区不订阅，15 秒轮询兜底。 |
| 不移植 | 插件管理、DeepSeek 账号与访客管理、Node 堆/插件环境修复、可选通用 PTY 终端与快捷命令；不为对齐页面数量开放系统执行入口。 |

开发工作树中，`navigation.ts` 从 URL 片段解析当前视图；Vue 从该状态派生标题、当前导航链接和分区显隐，`#version` 属于概览。状态与日志控制器直接消费当前分区，不从 DOM 属性反推。若宿主拒绝纠正无效片段，保留完整单页，停止分区日志订阅。

本节取代前面有关版本页位置和局部 Vue 渲染的历史说明。四区、`#version`、授权回调、任务归属及现有密码策略保持兼容。管理页是编译后的单文件，不加载 CDN、第二前端运行时或运行时模板编译器。

## 验收边界与当前状态

- G0–G3 按已批准范围完成；真实手机 fnOS App 暂不纳入本轮验收，最终手机端复测仍待用户。fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不表述为已撤销或已修复。
- 官方 PyPI 直连 15 分钟无输出超时；真实阿里云 `qwenpaw==2.2.2.post1` 上游安装/卸载通过，不等于官方 PyPI 直连通过。
- 本文为静态源码契约盘点。候选和历史平台证据见 `docs/session-handoff-2026-11-04.md`、`docs/platform-validation-2026-11-03.md` 与 `docs/unified-gateway-migration-plan.md`；命令存在不代表本轮执行或通过。

项目已有回归入口包括：

- 管理/平台：`python3 scripts/test-platform-callbacks.py`、`python3 scripts/test-management-boundary.py`、`python3 scripts/test-supervisor-api.py`、`bash scripts/test-lifecycle.sh`、`bash scripts/test-config-callback.sh`、`bash scripts/test-venv-recovery.sh`、`python3 scripts/test-runtime-venv-contract.py`。
- 前端：`npm test --prefix web`、`npm run typecheck --prefix web`、`npm run test:e2e --prefix web`、`npm run test:e2e:gateway --prefix web`、`npm run test:e2e:gateway:webkit --prefix web`。
- 门禁：`STRICT_CI=1 bash scripts/lint.sh`、`bash scripts/preflight.sh`、`python3 scripts/gen-audit.py --check`。

`test-supervisor-api.py`、`test-venv-recovery.sh` 等依赖 release supervisor 的场景应先按脚本要求构建并隔离数据目录。CGI 专属测试入口已删除。运行本文件列出的命令不意味着已完成 fnOS 真机、手机 App 或生产发布验收。
