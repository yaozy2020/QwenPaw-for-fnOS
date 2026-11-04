# 管理层去脚本化整改计划与测试方案

- 初始计划日期：2026-11-03；本次进度更新：2026-11-04。
- 当前进度：P0–P5 本地实现及安全复核修复已完成；生产管理层已迁入 Rust/native CGI 薄桥，QwenPaw 本体未修改。P4 fnOS 网关/真机验收仍阻塞。初审整改后的最新本地包为 1.1.1 RC1 `build/output/qwenpaw-1.1.1-1-x86.fpk`，SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`，大小 2,071,727 字节；Rust 222+13、Node 30 及完整本地门禁通过。下文早期 `p6-candidate` 摘要保留为整改前历史记录。候选不是生产可用声明，且不代表外部平台或发布门禁通过。
- Rust 206+13、严格 Clippy、lint、audit、自测、包内 API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 和 Chromium 1 项均通过。复用同一环境 `target` 的双构建 SHA 一致，但不是洁净房构建证明。包内 supervisor SHA256 `4969a976f1c13f9d20f835eecc48b222001eb63ec9925e7760927e2add12da79`；test helper SHA256 `dd59f19e53204fa9f81d4cab097c0d89af002a95d595cc7247cfb84584521c91`。
- 调查基线：先前工作树包含大量未提交的模块化、配置事务、安装恢复及管理层实现。必须按当前工作树判断并保留既有改动。本次只更新文档，不改实现，不重跑完整构建/测试，也不部署或操作真实数据。未 commit、push、发布或部署。

## 1. 目标、范围与非目标

### 1.1 已确认的目标

保留 Launcher 主线、外壳/内核版本双轨、Rust 监督者和 TypeScript/Preact 前端；让自研生产管理层退出 Python 和承载业务的 Shell，实现职责模块化、单一写入权威、事务化及可恢复执行。

允许 Rust 和 TypeScript。不是要求所有代码必须 Rust，也不是把 Rust 换成 Go。Rust 负责系统管理核心，TS 负责前端、客户端协议和合适的工程工具；不增加与 Rust 职责重叠的 Node 管理服务，不形成第二套安装、配置或恢复实现。

### 1.2 生产管理层的退出标准

1. JSON、URL、版本列表、配置、状态转换、安装编排、生命周期与恢复逻辑，不由自研 Python 或业务 Shell 实现。
2. fnOS 必需的 `cmd/*` 保留为薄入口，只定位原生程序、传递固定子命令并 `exec`；不承载业务判断、配置写入、网络请求、进程清扫或安装策略。
3. CGI 仅作为迁移期兼容入口；只有网关路径、鉴权和平台兼容均通过真机验收后，才可退役。当前尚未达到此条件。
4. 管理写操作经共享 Rust 管理核心；在线平台命令和 HTTP 入口复用管理规则。不得把 loopback HTTP 或私有控制 UDS 误称为 fnOS 网关传输。
5. 不因控制台关闭、请求断连、平台重试或管理器重启重复执行、误认成功、误杀进程或丢失恢复依据。

### 1.3 不在本轮范围

- QwenPaw 本体源码、内部架构、插件、多智能体能力和数据格式迁移。
- 删除 QwenPaw 所需的 Python/Node 运行依赖；外部运行时只通过 Rust `RuntimeAdapter` 的固定 argv 调用。
- 将构建、测试、打包和 CI 工具全部改写为 Rust/TS。工程侧 Python/Shell 可保留，但不得进入生产管理业务链路。
- 更换前端框架、视觉重设计、引入数据库/消息队列/容器编排，或代理内核全部业务流量。

`RuntimeAdapter` 可调用 venv、pip、QwenPaw CLI 及固定 metadata/import 探针；这些调用属于受管运行时协议，不是管理业务。不得把 JSON、联网查询或策略藏进 `python -c`。

## 2. 当前工作树调查结论

以下按当前源码描述静态实现；命令可用不代表本轮执行。平台行为见 `platform-validation-2026-11-03.md`。

| 当前实现 | 源码位置 | 边界/未完成项 |
| --- | --- | --- |
| Rust 启动装配 `ManagementCore`；CLI 有 `serve`、`service`、`platform`、`doctor --json`、`web api-cgi`、`web index-cgi` | `supervisor/src/main.rs`、`cli.rs`、`management.rs` | 本地管理核心已实现；不等于 fnOS 网关验收完成 |
| 管理 HTTP 仅监听 loopback；HTTP 与私有平台控制入口共用管理核心 | `supervisor/src/` | 私有控制通道不公开，也不是统一网关 Socket |
| 私有 control UDS 0600 且用 `SO_PEERCRED` 核验 UID | `supervisor/src/control.rs` | 仅证明本地私有 IPC 设计，不证明网关身份或管理员授权 |
| 生命周期、配置、安装/卸载、镜像等回调转发 Rust 固定子命令 | `cmd/`、`supervisor/src/platform.rs`、`service.rs` | fnOS 回调阶段和实际平台重试语义仍待真机验证 |
| API CGI 为薄入口，由 Rust 处理兼容 action、方法/CSRF 和 loopback token 转发；index CGI 由 Rust 提供静态服务 | `app/ui/`、`supervisor/src/api_cgi.rs`、`index_cgi.rs` | 两个 CGI 仍随包使用；API CGI 是迁移期兼容桥，未退役，且不是网关认证 |
| 版本索引由 Rust HTTP/HTML 解析；运行时由固定 argv 的 `RuntimeAdapter` 管理 | `supervisor/src/version_index.rs`、`runtime.rs` | Python 仅作为受管 QwenPaw/venv 运行依赖，不承担管理策略 |
| 激活暂存 venv 前重定位 console-script shebang | `supervisor/src/install/io.rs` | 最终候选包内真实 venv 契约测试通过；不代表真实 PyPI 上游包已验证 |
| 前端 API 客户端、TypeScript 检查和 Playwright E2E 已存在 | `web/src/api.ts`、`web/package.json`、`web/e2e/control-panel.spec.ts` | Chromium 1 项通过；仍未验证 fnOS App/WebKit/网关鉴权 |
| Shell 管理库已删除；审计、边界和事务测试入口已存在 | `cmd/`、`scripts/` | 候选 FPK 物料摘要及包内测试已验证；真机验证仍未关闭 |

必须保留的运行时能力包括安装事务恢复、配置快照恢复、关闭门禁、终态收尾互斥、进程组清扫、桌面入口定点更新及前端 `seq + instance` 任务归属判断。`docs/backend-design.md` 是历史提案，不作为当前实现依据。


## 3. 推荐架构与职责边界

当前工作树路径：

```text
浏览器
  └── app/ui/index.cgi → Rust 静态服务
      └── app/ui/api.cgi → Rust CGI 兼容处理 → loopback HTTP / private control UDS
                                               └── ManagementCore
                                                   ├── 配置、安装、恢复、生命周期事务
                                                   └── RuntimeAdapter → 受管 QwenPaw 运行时

fnOS cmd 薄入口 → Rust platform/service 子命令 → private control UDS（在线）
                                          └── ManagementCore
```

`api.cgi` 和 `index.cgi` 仍随包使用。`supervisor-control.sock` 是应用私有控制通道，不是 fnOS 网关 Socket；loopback HTTP 也不是统一网关。

目标平台路径（当前未接入、未验收）：

```text
浏览器 / fnOS App
  → fnOS 登录与应用授权
  → 统一网关 /app/qwenpaw
  → gatewaySocket（受验证的网关身份上下文）
  → Rust 管理 HTTP 服务 / ManagementCore
```

不得把目标路径画成当前已实现架构，也不得在真机网关、鉴权与最低版本验证完成前退役 CGI。

### 3.1 当前模块边界

实现保持单 crate，不拆 workspace 或微服务。当前源码职责如下：

- `supervisor/src/management.rs`：传输无关的共享管理核心，HTTP 与私有控制通道复用管理规则。
- `platform.rs`、`service.rs`、`cli.rs`：分别承接平台回调、服务控制及 Rust CLI；平台 Shell 入口只定位二进制并转发固定子命令。
- `runtime.rs`、`install/`：受管程序固定 argv/env、探针和超时，以及安装编排、venv 事务、进程和 I/O。
- `runtime_settings.rs`、`settings_transaction.rs`、`desktop.rs`：配置事务、恢复快照和桌面入口定点更新。
- `api/`、`api_cgi.rs`、`index_cgi.rs`、`control.rs`：loopback HTTP API、迁移期 CGI 兼容处理、静态资源及 private control UDS。尚无 fnOS gatewaySocket 路由。
- `web/src/api.ts`、类型定义及任务模块：封装现有 api.cgi 兼容地址、请求、错误与任务身份；网关前缀迁移仍待真机验收后另行决定。

只在 HTTP、CLI、平台命令和恢复入口确实共用的规则上复用管理核心，不泛化成插件框架。

### 3.2 当前原生 CLI

当前 CLI 已实现 `serve`、`service`、`platform`、`doctor --json`、`web api-cgi` 与 `web index-cgi` 子命令。具体命令参数、状态码和输出以 `supervisor/src/cli.rs`、`platform.rs` 与 `service.rs` 为准；本节不将旧提案中的示例命令名视为当前 API 契约。

- `cmd/main` 和平台回调转发固定子命令；配置、安装、停止和卸载业务由 Rust 处理。
- 管理 HTTP 服务状态与内核运行状态是不同概念；内核未安装/损坏不应被表述为管理页面已通过平台验收。
- 诊断和本地 CLI 的存在不证明 fnOS 生命周期窗口、回调顺序或应用中心错误呈现已验证。

### 3.3 所有权与传输边界

源码有监督者所有权锁、服务控制、进程内操作协调和私有控制 UDS。具体锁顺序、CLI 行为及异常状态以当前 `supervisor/src/` 实现为准；这里不再把旧计划中的额外平台生命周期锁或离线执行协议写成已实现事实。

- loopback HTTP 仅供本机管理兼容路径；私有 control UDS 仅供受信的平台私有命令，不接受浏览器身份。
- 服务仍持有所有权但 IPC 不可达时不得绕过管理核心直接写盘。
- 状态和诊断不得隐式启动、修配置或消除恢复证据；恢复门禁失败时保留证据并阻断变更。
- fnOS 网关 UDS 是目标平台传输，当前尚未实现/验收；不得与 `supervisor-control.sock` 混称。

### 3.4 当前管理接口与任务协议

- Web 客户端仍通过 api.cgi 兼容路径进入 loopback 管理 API；目标 `/app/qwenpaw` 网关前缀及 gatewaySocket 路径未接入。
- 原生 CGI 处理兼容 action、方法、CSRF 和请求转发；这不是 fnOS 登录态或管理员角色授权的替代证明。
- 安装任务依赖 `seq + instance` 归属判定；客户端在无法确认提交结果时不得猜测成功或重复提交。
- HTTP/CGI 兼容响应和状态码以当前 handler 为准。旧计划中的持久化 `operation_id`/请求指纹和统一新 API 语义属于未验收的后续设计，不可写为当前契约。
- 安装事务 journal 与配置恢复记录支持可判定恢复；如需扩展到其他操作，仍须另行定义并验证，不能假定所有操作共用同一种持久化记录。
- 状态、日志和事件观察不得代替写操作协调；busy、恢复阻断与失败状态应按现有响应契约如实呈现。

## 4. 事务及数据安全设计

### 4.1 持久化布局兼容

第一轮保留 `service_port`、`qwenpaw-log-level`、`pip-mirror`、`qwenpaw-version`、现有 venv 路径及事务文件的兼容读取，不同时切换为全新聚合配置格式。

新增记录必须带 schema、operation_id/tx_id、操作类型和可判定阶段；写入使用同文件系统临时文件、文件 fsync、rename 和父目录 fsync。未知 schema、记录损坏、路径不在允许根内时 fail-closed，保留原始证据。

跨外壳升级须专门验证旧 journal 恢复：旧桌面配置整文件快照不能最终覆盖新包的网关入口。应先用兼容恢复器完成旧事务，再按新模板定点迁移管理入口；不得覆盖用户权限设置或其他桌面入口。

### 4.2 各类操作的事务边界

| 操作 | 成功条件 | 失败/中断处理 |
| --- | --- | --- |
| 安装/升级/精确修复 | 目标版本校验、进程与健康检查、持久化提交决定、版本钉一致 | 未提交恢复旧环境；已提交完成新环境收尾；无法确认则停止并保留日志/备份 |
| 端口/日志级别 | 文件、管理器内存、桌面入口一致；需要重启时服务真实健康 | 恢复旧字节/权限和内存，确认旧服务；恢复失败阻断后续写操作 |
| 镜像设置 | URL 规则通过、原子写入、后续请求确实消费该值 | 写失败保留旧值；进行中安装使用受理时的镜像快照，不中途换源 |
| 版本钉 | 精确版本格式、操作互斥、原子持久化 | 不允许 latest/beta 作为 pin，不因无效 pin 自动追最新 |
| 重置认证 | 备份、重启、健康检查和清理决定可恢复 | 恢复原文件后才可拉起旧实例；不能以裸备份文件存在推断最终结果 |
| 卸载内核、保留数据 | 内核已停、runtime/pin 移除状态可判定 | 中断可继续清理或按记录恢复；用户数据与认证不删 |
| 完整卸载并删数据 | 明确 remove 选择、目标清单核验、服务及后代停止 | 不可逆操作，不能宣称事务回滚；部分失败记录剩余清单并如实报错 |

不把内核程序回滚等同于用户数据回滚。管理器不负责 QwenPaw 内部数据迁移；安装失败自动回退只覆盖明确受控的运行环境、配置和启动状态。

### 4.3 安装和恢复纪律

- 首次安装外壳时不为管理器创建 Python venv，不联网升级 pip；内核未安装时管理页面可用。
- 已有正常内核的外壳升级不得执行内核升级，版本保持不变。
- 需要修复缺损环境时保留按合法 pin 精确恢复的语义，走同一安装事务；无合法 pin 不猜版本、不追最新。此修复可能需要网络，不能宣称所有修复都能离线完成。
- 初始化只允许针对明确的新数据目录；有用户数据时不得重新运行会覆盖数据的初始化。此约束放入适配器/管理核心，而非遗留脚本。
- 保留取消、shutdown latch、唯一终态写入者以及安全收尾完成前持续 busy 的规则。
- 真实 venv 必须验证 staged→active 后的 `sys.prefix`、import、console-script shebang 和后续 pip 操作。现有复制/改名方案不能仅凭假解释器通过就认定安全。
- 默认不引入多版本槽。若真实 venv 路径验证失败，暂停安装迁移，补充稳定目录/活动指针等最小修正设计及旧布局兼容测试，不跳过门禁，也不直接照搬 Node 的目录切换。

### 4.4 路径、进程与权限

- TRIM 路径优先；安装、服务、向导、卸载、日志及清理使用同一解析器。
- 保留平台合法的 `target` 等软链语义，但数据删除只允许已解析并验证的应用专属根；拒绝空路径、`/`、共享根和越界路径。
- 卸载未知选项默认保留；删除计划枚举每个目录，不能使用全局通配或对共享根递归 chown/chmod。
- PID 文件只是提示；发信号前核对 PID、starttime、应用身份、进程组，不能只匹配名字或 cmdline 含 `pip`。
- 管理器升级后旧 exe 的 deleted 状态也纳入身份策略，既能安全停止旧实例，也不能宽松到误杀同名进程。
- 敏感 token/私有控制 Socket/日志的权限最小化；不把管理 token 发给浏览器或写到 argv、URL、错误日志。

## 5. 网关、静态服务和安全边界

采用 fnOS 统一网关作为目标方案。当前 `manifest` 声明最低 1.0.0，但本轮没有验证此最低版本的网关能力；不能直接把“文档有接口”当作所有已声明版本均支持。

P0 必须实测：Socket 路径和生命周期、网关进程身份/访问权限、前缀是否剥离、用户 Header 是否覆盖客户端同名 Header、应用授权与管理员权限、HTTPS/移动端 Cookie、SSE 行为、桌面配置更新是否需要重注册。

安全设计：

1. 网关登录校验不代替业务授权。管理 API、日志和敏感状态要求管理员权限；非管理员即使知道 URL 也不能调用。
2. `X-Trim-*` 只在经验证的网关传输上作为身份上下文；回环 TCP或 private control UDS 不因携带同名 Header 自动获得管理员权限。
3. private control UDS 仅允许应用/受信的平台调用者，不能复用浏览器鉴权策略。
4. CSRF 签发与校验迁入 Rust，使用服务签发、绑定浏览器上下文的 token；Cookie 限定路径和 SameSite，在 HTTPS 环境设置 Secure。不能重新引入“Host 必须等于 Origin”的反代误杀。
5. 变更只允许 POST 等约定方法；GET/HEAD 无写入副作用；未知路由 404，方法错误 405 并返回 Allow；不开放通配 CORS。
6. 新静态服务仅提供构建物白名单，拒绝编码/双重编码穿越、NUL、越界软链，不能把整个 target/var 暴露出来。
7. 保留 gzip/br 协商，正确处理 q=0、Vary、Content-Type、HEAD 和不存在资源。HTML 不长期缓存旧 API 地址；压缩物必须与原文一致。
8. Socket 路径已有普通文件或未知属主时失败，不无条件 unlink；只清理经确认属于本服务的陈旧 Socket。
9. 版本索引 HTTP 请求需 TLS 验证、连接/总超时、响应上限和重定向限制。不以 curl 子进程替代原生网络实现，也不手写 TLS。
10. 镜像策略保持管理员显式配置内网镜像的能力；同时拒绝非法 scheme、参数注入和凭据泄露。不得以安全整改为由静默把自定义源改成官方源。

网关验证失败时，暂停 CGI 退役，不自动新增公开端口或提高最低系统版本。需显式确认兼容范围后再推进。过渡兼容桥可以是薄入口调用 Rust 的 native CGI 子命令；不保留 Shell/Python 的业务回退。

## 6. 分阶段实施计划

下列工作清单保留原始计划和验收目标，不应被理解为当前尚未开始的实现任务。当前状态按本地源码实现与外部验收分开记录；命令/测试入口存在也不代表执行或通过。每阶段独立可审查、可回退，不自动 commit/push 或修改部署。

| 阶段 | 当前状态 |
| --- | --- |
| P0 | 本地契约盘点与测试入口实现已完成；fnOS 网关能力、回调窗口和鉴权仍需真机证据。 |
| P1 | 本地管理核心、所有权协调及原生 CLI 已实现；最终候选包内 API/lifecycle 120 并发及相关自测通过，平台并发/故障语义仍需真机验证。 |
| P2 | 本地配置、镜像、向导与生命周期入口已迁入 Rust；平台回调真实语义未验证。 |
| P3 | 本地安装、恢复、卸载和受管运行时路径已实现；最终候选七点恢复与真实 venv 契约测试通过，真实 PyPI 上游及更广泛平台故障矩阵仍未验证。 |
| P4 | 本地 API/静态 CGI 兼容、HTTP、私有 control UDS 和 TS 客户端已存在；fnOS gatewaySocket/登录与管理员授权/移动 App/最低版本验收阻塞，CGI 未退役。 |
| P5 | TS 客户端、typecheck 与 Playwright E2E 入口已存在；最终候选 Chromium 1 项通过，fnOS App/WebKit 未验证。 |
| P6 | 最终候选 FPK、本地物料审计、包内测试和 SHA 摘要已完成；真机升级/回退及平台、上游验证未完成，不构成生产发布结论。 |

### P0：冻结契约与验证平台可行性

本地契约盘点、当前实现清单和测试入口已在工作树中形成；真机网关、回调时机、鉴权与最低版本证据仍阻塞。以下原计划项作为验收目标记录，不表示本轮执行：

计划工作项：

- 归档当前工作区的差异、可恢复开发基线，以及已发布旧包的版本和 SHA256；不得清理已有未提交改动。
- 列出全部管理入口、写入文件、外部命令、错误码、默认值、向导回显和保留数据语义。
- 用独立 fnOS 测试安装验证第 5 节网关边界及实际平台超时。
- 验证 install_init 时原生程序是否已解包，upgrade 各阶段可执行路径，以及 uninstall_callback 时 target 是否仍存在。
- 原生卸载应放在“用户选择已确认、原生程序尚可执行”的阶段。若不存在此窗口，先设计并验证私有临时 helper 的存活和清理协议；没有可用原生执行窗口就阻塞该里程碑，不能回退 Shell 删除业务。
- 先修测试可信度：任何前置失败、端口不可用、就绪超时、未触发注入点都必须失败；健康端点由受管替身自身提供。

交付：契约清单、平台能力记录、测试基线及风险关闭结果。

退出标准：平台执行时机和鉴权路径有真机证据；现有行为被测试明确表达；不能证明的兼容项列为阻塞，而非默认通过。

### P1：提取传输无关管理核心和所有权协调

本地管理核心、服务装配、操作协调及原生 CLI 已实现。下列原计划工作/交付/退出标准描述实现目标，不能据此推断并发与故障验收已运行或通过。

计划工作项：

- 从 `main.rs` 分离配置/日志初始化、服务装配、恢复、传输和信号处理。
- 从 `runtime_settings.rs`、API handler 提取管理上下文，保留现有事务算法，不先重写全部安装代码。
- 增加统一命令、结构化错误、进程内操作协调和在线/离线所有权。
- 固定生命周期锁、实例锁的获取/释放规则；实现就绪确认和 shutdown 预算。
- 首批 Rust 原生测试辅助进程替代假 Bash/Python 服务，不要求立刻重写所有工程测试驱动。

交付：可被 HTTP、CLI 和恢复入口复用的核心；原生 service/doctor 子命令。

退出标准：真实多进程 start/stop/restart 并发通过；只有一个管理核心所有者；诊断不阻塞于慢安装；停止后锁和目标后代均释放；无关进程存活。

回退边界：旧入口仍可调用兼容的无参数服务模式；不变更持久化布局。

### P2：收拢配置、镜像、向导和生命周期入口

本地配置、镜像、向导和生命周期管理路径已迁入 Rust；平台回调的真实运行时序和授权行为仍待 fnOS 验证。下列原计划项保留为实现目标/验收基准，不表示平台已验收。

计划工作项：

- 将 `config_init/upgrade_init` 的回显逻辑、`config_callback` 的 JSON/curl/Python 链迁入原生平台模块。
- 增加镜像读写原生管理命令，让 CGI 不再直接写 `pip-mirror`。
- 迁移 `cmd/main`、`supervisor_ctl.sh` 的进程身份、锁、启动就绪、停止、平台通知逻辑。
- 各文件只保留薄 exec 入口；`install_init/uninstall_init` 无需业务时保留明确 no-op，不能为减少脚本数而伪造逻辑。
- 配置操作在线走 private control UDS；离线按明确命令策略执行，不因服务请求失败而自行写文件。

交付：原生配置、镜像、向导和服务控制；薄平台入口。

退出标准：非法输入零写入；文件/内存/桌面同步；失败回滚；trace 中没有管理用 Python/curl/命令拼接；旧 CGI 只转发管理命令。

### P3：统一安装、精确修复、升级和卸载

本地安装/恢复/卸载路径及受管运行时适配已实现；最终候选的七点恢复和真实 venv 契约测试通过。真实 PyPI 上游包及全量平台崩溃故障矩阵仍未验证。下列原计划项及退出标准保留为验收基准，不表示所有测试已通过。

计划工作项：

- 将 `install_callback/upgrade_callback/lib.sh` 的环境判定、pin、镜像、init 和修复编排迁入同一安装服务。
- 将内核卸载和完整卸载的路径计划、停止、权限检查、错误通知迁入原生实现。
- 为认证重置、内核卸载等尚非完整 journal 的高风险操作补齐可判定的恢复记录，不能只依赖临时备份命名。
- 引入最小持久化任务记录，统一受理、取消、失败、恢复和终态；安装事务记录仍是环境恢复依据。
- 执行真实 venv/本地 wheel 路径验证，修复发现的问题后才迁移生产安装入口。
- 删除不再被调用的 `lib.sh/supervisor_ctl.sh` 生产物料；不能只“不调用但继续打包”。

交付：Web 与平台回调共用的安装/修复/卸载闭环。

退出标准：空外壳离线可装；健康环境升级不改版本；缺损环境按 pin 精确恢复；无 pin 明确失败；已有数据不重复 init；七个既有断点及扩展故障矩阵通过；全删只影响允许的测试清单。

回退边界：仍保留旧布局读取；有未决 journal 时先恢复，不能直接换旧管理器接管。

### P4：原生 Web/网关接入与管理用 Python 清零

当前状态：Rust 静态服务、管理 API、迁移期 CGI 兼容、CSRF 处理、私有 control UDS 和原生 HTTP/HTML 版本索引已在本地工作树实现；fnOS `gatewaySocket` 路由与网关登录/管理员授权未实现或未验收。以下保留为目标平台验收与剩余迁移清单：

- 在目标系统验证网关鉴权、CSRF、Header 身份上下文及最低版本能力；私有 control UDS 不替代该工作。
- 版本选择与降级判断由后端权威，TS 不维护第二套安装决策；该项按当前源码契约实现，仍需集成验证。
- 前端 API 地址脱离 `index.cgi/api.cgi`，迁移到经验证的稳定网关前缀；当前仍使用 CGI 兼容路径。
- 保留轮询可靠路径；现有 SSE 可测试后使用，不把新增 WebSocket 或 SSE 重构作为去脚本化前提。
- 只有网关、鉴权和目标平台兼容通过后，才改 `app/ui/config` 并停止打包旧 CGI；不反代 QwenPaw 本体。

目标交付：原生管理入口和完整安全边界，并在具备真机证据后退役生产 CGI；此目标尚未完成。

退出标准：未登录/非管理员/伪造 Header/CSRF 全部负例通过；Host 重写不误杀；手机/浏览器/外网 HTTPS 路径可用；内核损坏不影响管理页面；管理业务不再调用辅助 Python。

### P5：TS 客户端协议和前端测试门禁

当前状态：客户端 API、类型定义、`seq + instance` 任务归属逻辑、typecheck 配置和 Playwright E2E 入口已存在。以下是既有实现范围及后续验收重点，不是“工具尚未安装”的待办：

- 维护 API client、DTO、错误、轮询/重连与任务归属，保留当前视觉和已有 `installTask` 规则。
- 对受理响应丢失、跨实例、乱序响应、关闭重开、后台切换确认“未确认/恢复中”状态语义。
- 扩充真实浏览器闭环；纯逻辑测试与 DOM/网络测试各自负责，不重复堆叠同类用例。最终候选 Chromium 1 项通过；Node 纯逻辑测试/typecheck 的执行结果未在本次报告中列出，fnOS App 仍需真机独立验证。

目标交付：类型化客户端及浏览器验收证据。

退出标准：不误报成功、不重复提交、不被旧响应覆盖新任务；typecheck、现有前端回归和浏览器关键流程全绿。

### P6：打包门禁、兼容回退和候选交付

当前状态：本地最终候选已构建并通过物料审计、包内测试与摘要核对。候选为 `build/p6-candidate/qwenpaw-2.1.0-9-x86.fpk`（2,070,195 字节，SHA256 `b0a1fa9ec952eedcb02a1e9ad4d3e69aaf2e645760e0c1e7507ab695ac08413d`）；包内 supervisor SHA256 `4969a976f1c13f9d20f835eecc48b222001eb63ec9925e7760927e2add12da79`，test helper SHA256 `dd59f19e53204fa9f81d4cab097c0d89af002a95d595cc7247cfb84584521c91`。Rust 206+13、严格 Clippy、lint、audit、自测、包内 API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 及 Chromium 1 均通过。同环境复用 `target` 的双构建 SHA 一致，但不是洁净房可复现性证明。fnOS 真机升级/回退与其余平台门禁未完成；候选不是生产可用声明，也未发布。以下仍列出未关闭的发布与平台验收门禁。

计划工作项：

- 同步修改 `build.sh`、preflight、lint、audit-fpk、gen-audit 和 CI/Release 对 CGI、执行位、Shell 的假设。
- 保持单二进制优先；若增加原生 helper，明确加入 staging 和审计，不依赖 Cargo 自动复制。
- 物料清单由单一来源消费，覆盖路径、文件类型、模式与架构；归档先验成员再解包，拒绝越界项与未声明特殊项。
- 对最终 FPK 解出程序和薄入口重跑集成测试，比较被测二进制 SHA256 与包内物料。
- 在独立 fnOS 安装完成旧包升级、系统重启、候选回退和卸载保留/删除验收。
- 更新 README 的架构/入口/故障排查和自动生成审计说明；不把历史提案改成虚假现状。

交付：候选 FPK、SHA256、测试证据、兼容矩阵、已演练回退步骤。

退出标准：全部发布阻塞项通过；无旧生产管理业务；包内程序就是已测程序；没有未决事务或未披露兼容变化。

关键路径：`P0 → P1 → P2 → P3 → P4 → P6`。P5 的客户端基础可在协议稳定后并行，但网关浏览器验收依赖 P4。工期在 P0 明确真机可用性和兼容目标后估算，不以固定日期代替退出标准。

## 7. 测试现状与可信度整改

### 7.1 已有测试，可继承但不能扩大结论

仓库已有 Rust、Shell/Python 集成与边界检查，以及 Node/typecheck/Playwright 前端入口；具体命令见第 11 节。本次最终候选验证中，Rust 206+13、严格 Clippy、lint、audit、自测、包内 API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 与 Chromium 1 项均通过。完整候选路径、SHA 与复现性边界见交接文档。以上结果不证明 fnOS 真机网关、真实 PyPI 上游、WebKit 或 App 验收通过。

| 入口类别 | 代表入口 | 不证明什么 |
| --- | --- | --- |
| Rust 单元与集成 | `cargo test --locked`、`python3 scripts/test-supervisor-api.py` | 不证明 fnOS 网关、真实 App 授权或最终 FPK |
| 平台回调/CGI 与边界 | `scripts/test-platform-callbacks.py`、`test-native-api-cgi.py`、`test-native-index-cgi.py`、`test-management-boundary.py` | 不等于目标 fnOS 上的回调时序或网关鉴权 |
| 生命周期/事务/venv | `test-lifecycle.sh`、`test-config-callback.sh`、`test-venv-recovery.sh`、`test-runtime-venv-contract.py` | 不证明断电持久性、真实 fnOS 或移动 App 行为 |
| 前端与浏览器 | `npm test --prefix web`、`npm run typecheck --prefix web`、`npm run test:e2e --prefix web` | 不证明 fnOS 网关和真实手机 App |

### 7.2 验收时的可信度要求

以下是后续验收要求，不是对当前脚本状态的缺陷判定：测试前置失败、端口不可用、就绪超时和未触发注入点必须导致失败；健康端点应由受管替身自身提供；记录被测二进制来源、摘要、夹具版本和随机根；确认所有目标后代退出且外围哨兵存活。任何脚本级缺陷均应先按当前工作树复核，再报告或修复。

本轮实际运行的最终候选验证及通过范围见本节开头与 `docs/session-handoff-2026-11-04.md`。本次文档更新未修改实现或测试。上列测试可信度要求仍适用于后续测试，不能将本地通过结果扩展为真机或上游验收。

## 8. 分层测试设计

### L0：架构与物料边界

- 生产白名单中无自研管理 `.py`，无 `lib.sh/supervisor_ctl.sh`，最终无旧 CGI。
- 薄入口允许固定路径定位和 exec，不允许 curl、pip、Python、sed写配置、rm/chown业务、进程匹配/清扫、循环重试业务。
- 原生 CommandRunner 对每次 argv、env 和目的做测试记录；禁止 Shell 拼接及管理用途解释器调用。允许列表只包括集中适配器中的受管运行时操作。
- 不对整个仓库简单 grep `python` 后判失败；工程工具、测试夹具和受管程序是明确排除项。
- HTTP handler 不直接写配置或另起安装器；共享管理层不依赖 axum 的 Request/Response。

### L1：Rust 单元和组件测试

- 路径：TRIM 缺失/空值、多共享路径、重复斜杠、空段、空格/中文、非法根、软链与越界。
- 输入：端口上下界/保留端口/动态管理端口，所有合法日志级别，未知字段、错误类型、超限 body。
- 版本：当前支持的 PEP 440 行为、stable/beta/精确版本、post/dev/rc/local/epoch既有案例，拒绝 requirement/URL/路径/选项注入。迁移不暗中扩充或缩小格式。
- 镜像：预设映射、自定义 URL、显式空值、非法 scheme/字符、持久化优先级；运行中安装冻结镜像快照。
- Simple Index：wheel/sdist、URL编码、HTML实体、重复版本、相对链接、空列表、非目标包及恶意/超大响应。
- 文件事务：原字节、原权限、原文件不存在、提交/回滚/重复恢复、未知 schema和路径篡改。
- 任务：接单/取消/收尾竞态、幂等键重放、不同参数同键、终态唯一写入、重启后的身份和结果判定。

关键纯逻辑应有分支/边界覆盖，不以总覆盖率数字替代事务不变量。

### L2：真实原生 CLI、HTTP 与 UDS 黑盒

测试启动真实被测管理器和原生替身进程，不直接调用 handler冒充完整接口。

- 每条路由覆盖正确身份、缺失身份、错误身份、正确/错误方法和请求形状。
- 同时 20 个独立 CLI 竞争 start/start、restart/start、restart/stop；每种组合建议重复 100 轮，断言唯一服务、无死锁、退出码正确。
- 在线 API 与离线命令竞争；服务存活但 IPC不可达时禁止离线写入；恢复和卸载与 start 并发不得互相穿越。
- 慢 pip、慢 metadata、慢健康检查、慢终态回调期间并发访问 status/logs/events；只读接口有界，写操作如实冲突。
- UDS 权限、伪造网关头、陈旧 Socket、普通文件占位、服务重启、客户端半包/断连/大请求。
- service start 以就绪确认而非 PID 存活为准；status 不改变 PID/UI文件的字节、mtime、inode。
- 所有服务、子孙进程、端口、FD、Socket 和临时树由测试夹具统一回收；失败也必须清理。

### L3：故障注入与恢复

- 组件层注入 write/fsync/rename/remove/权限错误，验证错误不被吞掉。
- 子进程层在命名点 SIGKILL，确认确实到达断点且确实因 SIGKILL退出；重启恢复两次检查幂等。
- 使用独立临时目录和原生替身，不对已部署服务做故障演练。
- SIGKILL验证的是进程崩溃，不是供电丢失。真实断电持久性只在可丢弃 VM/测试盘快照环境验证，不能以 SIGKILL代称断电测试。

### L4：真实运行时适配契约

不修改 QwenPaw 本体，但管理器对外部运行时的调用需要验证：

- 用真实 Python 创建 venv，使用受控本地 wheel/index，验证路径、console-script、import、精确版本和失败回退；禁止退回互联网补包。
- 对 active/staged/backup 中各放唯一代际标识，确认 staging 安装不改 active、回退能恢复实际执行环境。
- 受管原生替身提供版本和健康端点，记录实际 argv/env、启动次数、init次数和所有后代PID。
- 发布前在独立 fnOS 实例选择一个明确的上游版本完成下载、启动、停止、改配置和管理器恢复冒烟；不需要真实模型账号或对话，不扩展到上游内部功能验收。
- 如果真实上游冒烟因网络或版本不可得而未完成，明确保留发布阻塞或请求用户接受限制，不能只用替身冒充端到端完成。

### L5：TS 与浏览器

Node 纯逻辑测试、独立 typecheck 与 Playwright 浏览器测试入口均已存在。最终候选 Chromium 1 项通过；Node 纯逻辑测试与 typecheck 是否运行不在本次报告范围内，不据此推断。真实 WebKit、fnOS App、网关登录与管理员授权仍须单独在目标平台验证。

- Chromium覆盖完整关键流程，WebKit覆盖移动端相关路径；真实 fnOS App必须另测，不能用WebKit模拟器替代结论。
- 页面经真实管理HTTP读取状态、设置镜像、提交配置、安装/取消任务、读取日志、卸载内核；与后台/落盘状态交叉断言。
- 受理后响应丢失、首拍已终态、busy附加、同seq跨instance、旧响应乱序、重复点击、两标签页和两设备。
- 关闭重开、最小化、hidden→visible、离线/恢复、登录过期、403/409/502、非JSON/截断JSON。
- HTML/日志中的特殊字符不能执行脚本；错误不静默；不编造百分比；不把“状态未知”显示为成功或内核未安装。
- 保留现有键盘、焦点、确认和移动端点击能力，不在本轮重做视觉风格。

### L6：fnOS 真机与最终 FPK

至少覆盖当前声明最低兼容目标和一套当前目标系统；最低版本无法取得或网关不可用时，标记兼容性阻塞。

- 通过应用中心实际安装最终FPK，验证TRIM、运行用户、权限、向导回显、失败退出和通知。
- 新装外壳→未装内核仍可管理→选择版本→启动→改设置→停/启→仅卸载内核→重新安装。
- 从上一稳定包及当前已有事务化候选升级，验证版本、用户数据、认证、管理入口和桌面入口保持。
- 正常重启系统、管理器崩溃后接管、安装中平台停止、外壳升级重启、失联后恢复。
- 管理员/非管理员/未登录，NAS本地域名、HTTPS反代和手机App的实际授权链。
- 卸载保留后重装，及隔离数据下完整删除；外围哨兵目录必须保留。
- 完成已知SHA256旧包的回退演练；只有确认兼容的schema/layout才允许直接回退。

## 9. 关键用例矩阵

下列均为计划用例，未表示本轮已执行。P0/P1是发布阻塞优先级，不是第6节阶段编号。

| 编号 | 优先级 | 场景与关键断言 |
| --- | --- | --- |
| M01 | P0 | 无系统Python/curl可供管理业务使用时，管理页面、版本查询、配置和诊断仍正常；适配器调用单独允许 |
| M02 | P0 | 首装断网：外壳可用、不创建管理venv、不运行pip升级、不假报内核已安装 |
| M03 | P0 | 健康旧环境升级外壳：内核版本/数据不变，不发生隐式pip升级 |
| M04 | P0 | 缺损环境+合法pin：精确恢复；无效/缺pin拒绝追最新，管理诊断仍可用 |
| M05 | P0 | 安装失败、版本不符、健康失败、pin写失败：实际旧环境、pin和旧服务一起恢复 |
| M06 | P0 | API/CLI/平台重试并发：仅一张受理单、一名写入者，不停掉无关或旧的可用实例 |
| M07 | P0 | cancel/shutdown发生在PID登记前、pip中、spawn后、终态回调中：不复活、不重复终态、不提前释放busy |
| M08 | P0 | 批量端口/日志配置成功、单字段、空patch、同值：正确持久化，无需重启时PID不变 |
| M09 | P0 | 配置任一写入、桌面同步、健康失败：恢复旧字节/权限/内存；恢复失败保留journal并阻止写操作 |
| M10 | P0 | 旧设置journal跨FPK升级：恢复后不把新console入口退回旧CGI，不覆盖用户授权设置 |
| M11 | P0 | 镜像保存并切设备查询一致；本次安装使用受理镜像，查询与pip同源，非法值不静默回官方 |
| M12 | P0 | 缺/错token、伪造X-Trim、非管理员、无CSRF、错误方法：真实HTTP拒绝且零副作用 |
| M13 | P0 | 网关Host重写、路径前缀、HTTPS Cookie、App后台：合法请求可用，安装不随页面销毁 |
| M14 | P0 | stale PID、PID复用、同名进程、旧exe deleted、特殊comm：不误杀，真实所属进程可管理 |
| M15 | P0 | 同组、双setsid、TERM免疫、宽限期继续派生的子孙：目标树清理，无关哨兵存活 |
| M16 | P0 | 未知schema、截断journal、缺backup、篡改路径：不擅自删证据/启动内核，允许只读诊断 |
| M17 | P0 | 卸载keep：数据/认证保留；remove：仅删清单；未知选项默认keep；部分删除失败如实报告 |
| M18 | P0 | 原生卸载执行窗口：目标程序可执行，回调重试幂等，不依赖已被平台删除的target |
| M19 | P0 | 真实venv staged→active后执行console-script、import和pip；回退后再次执行仍正确 |
| M20 | P0 | 最终FPK物料匹配测试摘要；旧CGI/业务Shell/管理Python不存在；gz/br解压等于HTML |
| M21 | P1 | 前端响应丢失、乱序、跨instance、关闭重开：不误认任务，不自动重复安装 |
| M22 | P1 | 日志/错误注入HTML、UTF-8截断、大日志：无XSS/崩溃、读取有界、敏感数据脱敏 |
| M23 | P1 | SSE首帧/心跳/重连/慢订阅者；轮询始终可用，不重复弹出终态 |
| M24 | P1 | 持续并发只读+慢安装：API延迟有界、线程/FD/内存无持续泄漏 |

### 9.1 安装事务的最低崩溃矩阵

沿用当前七点，并扩展日志更新之间的窄窗口。判断以持久化提交决定为准，不能仅根据断点名称推断。

| 中断位置 | 恢复预期 |
| --- | --- |
| staging准备完成、活动环境未动 | 保留/恢复旧代；首次安装仍未安装 |
| active→backup之后、journal尚未更新 | 从磁盘事实与记录恢复旧代，不丢备份 |
| backup阶段记录完成、尚未激活 | 恢复旧代 |
| 新代激活之后、journal尚未更新 | 停新进程后恢复旧代 |
| 新进程已健康、提交决定未持久化 | 停新进程，恢复旧环境与旧pin |
| 提交决定已持久化、pin未写完 | 按已提交记录完成新代pin和收尾；不能回滚已提交事务 |
| 当前after_pin_written/after_commit | 现有实现此前已记录Committed，保留新代并完成清理 |
| 恢复过程中再次SIGKILL | 再次恢复结果相同，无新增误删或重复init |
| 备份缺失、路径越界、未知schema | 停止并recovery_required，保留记录和可用证据 |

每个点分别覆盖首次安装、已有环境升级、exact与渠道解析、pin原本不存在；检查文件、实际服务版本、进程集合、版本钉、任务终态和残留，而不只检查一个标记文件。

### 9.2 配置及其他事务矩阵

对端口、日志、桌面、镜像、认证、卸载记录的“日志持久化后、每次写入后、重启前后、健康通过后、提交/清理前后”执行中断及I/O失败注入。

对已提交后清理失败，报告“业务已提交但清理待恢复”，不能把已提交结果伪装成完全回滚；对未提交失败恢复旧状态。日志损坏或回滚失败时，读接口可用，所有变更被阻止。

## 10. 测试环境、工具与依赖

### 10.1 隔离原则

- 默认使用临时TRIM根、唯一operation_id和动态端口，不指向`/var/apps/qwenpaw`的真实部署。
- 原生替身支持：metadata、pip成功/失败/挂起、健康服务、版本、init计数、后代进程树、信号免疫和受控故障。
- 测试目录隔离不能隔离/proc；所有发信号对象都必须有夹具登记身份，断言外围哨兵不受影响。
- 网络大多使用本地HTTP/TLS/index夹具；测试CA仅用于测试，不关闭生产TLS验证。
- 磁盘满、只读、跨卷、真实断电等需要专用VM/测试盘；没有环境就记录未验证，不使用生产目录制造错误。
- 保留失败日志供定位，但不得记录真实凭据。测试结束回收自产进程/目录/日志，保留原有用户文件。

### 10.2 当前依赖说明

本节的初始依赖调查已过时：当前工作树已在 Rust 中实现版本索引 HTTP/HTML 解析与私有 UDS，Web 侧已有 TypeScript typecheck 和 Playwright E2E 配置。实际依赖及版本以 `supervisor/Cargo.toml`、`supervisor/Cargo.lock`、`web/package.json` 和 `web/package-lock.json` 为准；此处不再列出新增依赖建议，也不表示相关命令或平台行为已在本轮验证。

### 10.3 性能与资源验收建议

以下为拟定验收目标，不是已测结果：

- 固定测试机、10个并发客户端读取状态，含后台慢安装，管理本机API P95≤500ms、P99≤1s；不把外部网络下载时间算入状态接口。
- 竞争写操作应在1s内明确受理或冲突，不排队到不可解释的长时间等待。
- 停止总时长必须小于真机测得的平台外层宽限；内层取消+恢复收尾+进程树清扫+余量不能超过该宽限，不能只检查脚本常量。
- 建议24小时混合查询/日志/连接重建稳定性演练；预热后FD、线程和任务数应回到稳定区间，记录RSS曲线，不把操作系统缓存当内存泄漏。
- 包体、启动时长、空闲CPU/RSS与基线同环境比较；TLS/解析依赖带来的增量需报告，未经测量不承诺“仍约1MB”。

## 11. 命令与发布门禁

### 11.1 当前已存在的命令

以下列出仓库标准基线命令；本轮实际执行并通过的门禁及专项测试见第 7.1 节与 `docs/session-handoff-2026-11-04.md`，未列出的命令不能据此视为已执行：

```bash
# 仓库根目录
bash scripts/preflight.sh
python3 scripts/gen-audit.py --check
npm ci --prefix web --no-audit --no-fund
STRICT_CI=1 bash scripts/lint.sh
npm test --prefix web

# supervisor/目录
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release

# 回到仓库根；后两项需要release二进制
bash scripts/test-lifecycle.sh
bash scripts/test-config-callback.sh
bash scripts/test-venv-recovery.sh
python3 scripts/test-supervisor-api.py
bash build.sh
bash scripts/audit-fpk.sh build/output/qwenpaw-<manifest-version>-x86.fpk
```

现有lint会运行部分回归/前端测试；编排时避免无新变化地重复执行。`bash build.sh`并不等于完整test/clippy/lint门禁。

### 11.2 现有专项检查入口

以下列出专项入口；本轮对最终候选执行并通过的 API/lifecycle、恢复、native API/index/platform、真实 venv 与 Chromium 检查见第 7.1 节。其余命令不能仅因列在此处而视为已运行；与 release supervisor 相关的脚本按其自身要求先构建并使用隔离数据目录：

```bash
python3 scripts/test-platform-callbacks.py
python3 scripts/test-native-api-cgi.py
python3 scripts/test-native-index-cgi.py
python3 scripts/test-management-boundary.py
python3 scripts/test-runtime-venv-contract.py
npm run typecheck --prefix web
npm run test:e2e --prefix web
```

前端纯逻辑测试由 `npm test --prefix web` 运行。旧计划里提议的 `cargo test --test management_cli/management_http/management_recovery/runtime_contract` 目标当前不作为已接入入口；不要将其列作可执行门禁。最终候选 FPK 的包内测试已完成；fnOS 真机安装、升级、兼容性与回退验收仍未完成。

### 11.3 新的发布阻塞顺序

1. 基线/版本/来源/物料契约检查，依赖按锁文件安装。
2. Rust单元与组件、Clippy；TS typecheck、前端纯逻辑测试与构建。
3. release构建，真实CLI/HTTP/UDS、恢复故障矩阵和本地真实venv契约。
4. 浏览器集成与架构退出检查。
5. 打包、归档成员完整审计、解压内容/执行位/目标架构和ABI检查。
6. 对包内程序及薄入口重跑黑盒核心用例，记录摘要。
7. 固定工具链、SOURCE_DATE_EPOCH等输入做双构建字节比较；另有洁净构建才可宣称洁净环境可复现。当前复用Cargo target的双构建不等于洁净房构建。
8. 使用同一个最终候选FPK完成fnOS安装/升级/权限/回退验收及必要上游适配冒烟。
9. 明确批准后才发布；保留tag→commit校验、不可变资产、上传后回读SHA256。不能只凭CI绿灯自动跳过真机门禁。

CI可并行独立检查，但Release必须聚合全部阻塞结果。修复门禁对旧CGI的硬编码时，迁移行为断言，不通过删测试获得绿灯。

## 12. 发布、回退和验收交付

### 12.1 候选发布切分

本节原候选 A/B/C 是旧计划的逻辑切分，不是当前发布安排。本地最终候选及包内验证已完成，具体路径与摘要见第 6 节；这不代表生产发布或真机验收完成。任何候选版本或 CGI 退役安排，都必须等相应平台验收门禁关闭后再确定。未完成退出标准不得提前对外宣称“管理层已无脚本业务”。

### 12.2 回退纪律

- 只有在未决事务恢复完成、并确认目标旧包兼容当前 schema/layout 后，才考虑回退到已知 SHA256 的旧包；不能确认兼容时不得直接回退。
- 回退前停止写操作、等待/恢复未决事务、确认 journal/schema/layout 可被目标版本理解。
- 旧包不理解新记录时，不能直接安装旧包；使用明确的导出/兼容转换或受测快照恢复步骤，无法证明则阻止直接回退。
- 真机回退未演练。程序/包回退不保证用户数据可逆；完整删除用户数据不可回滚，测试只在可丢弃数据副本进行。
- 网关变更回退需同时恢复入口注册、静态物料和服务配置；不保留自动 Shell 业务回退分支。
- 不在本轮执行部署、卸载、删除真实数据、commit、push、tag或发布。

### 12.3 最终验收清单

以下复选项是生产发布验收门禁，不是本地实现状态表。最终候选的本地物料审计、包内测试及摘要核对已通过；但 P4 真机网关和平台验收未完成，因此不能据此宣称达到生产发布条件。

- [ ] Rust/TS管理职责明确，无重叠Node管理服务。
- [ ] 生产Python管理业务退出；剩余Python调用全部属于集中受管运行时适配。
- [ ] Shell仅薄平台入口；旧业务lib和最终CGI不在包内。
- [ ] 安装、配置、镜像、生命周期和平台回调共用单一写入权威。
- [ ] 在线/离线互斥、任务幂等、关闭、恢复门禁及故障矩阵通过。
- [ ] 真实venv路径/脚本/import/pip验证通过。
- [ ] fnOS网关鉴权、管理员授权、CSRF、静态资源和移动端实测通过。
- [ ] 用户数据、认证、双轨版本和卸载保留语义不回退。
- [ ] TS typecheck和浏览器关键流程通过，不误报任务成功。
- [x] 本地候选FPK包内测试、物料审计、supervisor/helper/FPK摘要核对通过；同环境复用target的双构建SHA一致，不构成洁净房可复现性证明。
- [ ] fnOS真机新装、旧包升级、重启、故障恢复、回退和隔离卸载通过。
- [ ] 报告列明测试环境、包摘要、结果、未验证项和已知限制；没有静默skip的核心用例。

最终交付应包括实现、测试、CI/审计调整、候选FPK及SHA256、兼容矩阵和回退步骤。计划本身不等于上述交付已完成。

## 13. 参考与采用边界

- DSH调查固定提交：`c92f291205f5ebe133b67475145669579353d897`。
  - [管理服务装配与静态资源](https://github.com/shuangji66/dsh/blob/c92f291205f5ebe133b67475145669579353d897/backend/main.go)
  - [版本安装与切换](https://github.com/shuangji66/dsh/blob/c92f291205f5ebe133b67475145669579353d897/backend/server.go)
  - [受管进程管理](https://github.com/shuangji66/dsh/blob/c92f291205f5ebe133b67475145669579353d897/backend/dsh.go)
- [fnOS开发文档合集镜像：统一网关、生命周期和权限章节](https://github.com/ckcoding/fnnas-docs/blob/main/ALL_DOCS.md)。这是第三方文档镜像，不替代目标fnOS版本的官方契约和真机验证。
- 本地现状以`supervisor/src/`、`cmd/`、`app/ui/`、`web/`及实际CI/打包脚本为准。

采用DSH的控制面集中、常驻服务、原生参数调用和网关接入思路；不照搬其语言、Node目录切换或把保留旧程序版本误称为数据事务回滚。
