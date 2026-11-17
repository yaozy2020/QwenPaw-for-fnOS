# 管理层现状契约盘点

- 状态：按 2026-11-17 当前工作树实现更新；本文描述静态源码契约，不代表本轮执行了构建、测试或真机复测。
- 范围：fnOS 生命周期薄入口、Rust supervisor/管理核心、Gateway、私有控制 UDS、受管运行时及 Web 客户端。
- 当前工作树 manifest 为 `1.2.7-7`，并已按官方 JS SDK 要求声明 `micro_app=true`；当前候选及既往授权远端验收记录见交接文档。历史补丁清单同时合并各版本 `sha256` 与 `previous_sha256`，覆盖现场旧 venv；`-7` 已通过远程应用中心安装、内核启动和主 Web 深链验收，仍不写成生产发布。
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
- Rust `web_proxy` 将 `/app/qwenpaw/web/` 下的请求代理到上游 QwenPaw Web，保留上游路径与请求头中的 Bearer Authorization，支持流式 SSE；管理系统自身的 CSRF 校验与上游请求处理相互分离。WebSocket 明确返回 HTTP 501，不能宣称已支持 WebSocket。
- Gateway 总连接上限为 128；达到上限后为新连接等待最多 2 秒，有连接结束则继续服务，仍满员才返回 HTTP 503 与 `Retry-After: 1`。SSE 独立配额保持为 8。
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
- 保持 Preact，不迁移 Vue、不新增第二套框架。`authorizedDirsPanel.ts` 独立管理目录绑定、重新授权、解绑及原生打开；manifest 按官方要求声明 `micro_app=true`，使用 `@trimjs/web-app 0.5.0` 的 `TrimApp.ready()` 与 `openFileManager(path)`，桌面 Penpal 与手机 Flutter 由 SDK 统一适配；失败只追加 getAppMessage 是否为空、字段名和 appApi 等非敏感诊断，独立普通浏览器隐藏入口；面板显示真实返回，异常、false、success/ok=false 或非零 code 明确报错，null/undefined 不伪报已打开；`logPanel.ts` 独立管理单窗口日志合并、竞态、滚动、导出与清空；`settingsPanel.ts` 独立管理服务端口、Web 反代、镜像与认证；`versionPanel.ts` 独立管理版本查询、选择、比较与安装动作状态；`installCoordinator.ts` 负责安装确认和受理；`installProgressPanel.ts` 独占安装窗口、轮询、runId 与任务持久化；`statusPanel.ts` 负责状态竞态、SSE 快照和概览渲染；`installTask.mjs` 的 seq+instance 归属不变；`modal.ts` 和 `apiResult.ts` 共享弹窗与 API 结果类型；`pollingLifecycle.ts` 统一轮询的幂等启动、后台暂停、返回立即刷新与销毁清理；`navigation.ts` 独立管理移动导航、历史和监听器；`main.ts` 只负责页面编排，后续新交互应落在独立面板而非继续扩大主控制器。
- 手机端以固定底栏切换 `overview`（含版本）、`configuration`、`authorization`、`logs` 四个分区，页面底部预留安全区与导航空间；桌面端隐藏分区导航并展示完整单页。旧 `#version` 深链接映射到概览。“更新内容”入口、弹窗和请求已移除。状态以 SSE 为主、30 秒兜底，运行时长在本地每秒更新且不新增状态请求。日志取消翻页，首次读取 2000 行回看窗口，后续每 15 秒读取最新 100 行并按重叠合并，后台暂停；移动端日志终端填满剩余视口，支持自动滚底、上翻暂停、导出和清空。分区切换不再强制滚动，使用 160ms 淡入并支持减少动态效果。`config/resource` 声明 `trim.file.sharedAccess/userAccess/userAcl/path` 四个授权 scope。授权目录以 QwenPaw `project_dirs` 为登记来源，选择后由上游校验并直接保存，不另设权限门禁；PUT 只提交归一化 path 与非空 label，不得回传 GET 中的 `exists/nested_with/is_workspace` 等只读字段；失败固定显示在目录区域。Rust `POST /authorized-dirs/check` 仅用于重新授权后的独立权限复查。授权监听在导航前注册，启动时读取已落盘结果，回调地址使用同源绝对 URL；页面重建后可恢复 5 分钟内的落盘授权结果，继续 PUT 保存、显示目录并在成功后清除；面板显示授权、读取、保存、完成或错误阶段；回调页检查结果发布 HTTP 响应，失败时显示状态并停止自动关闭。嵌入回调由父页面收到结果后关闭，不刷新顶层控制台；原始 query 与仅含 `result.path` 的结构化结果均可进入绑定，state 校验和路径归一化保持不变；独立弹窗保留返回兜底。解绑不删除目录或文件。
- 备份入口复用上游 `/app/qwenpaw/web/backups`，不新增第二套备份后端。当前 FPK 以官方 v2.3.0 为基线携带 7 个后端文件及完整预构建 Console，动态预加载并发为 16，清单声明已核验的旧补丁哈希，未知本地修改仍拒绝覆盖。缺失父目录逐级检查并创建，非目录仍拒绝；完整载荷直接复制，不再下载其他版本源码或运行 npm/Vite；组长退出后的进程组清理修复仍保留，身份不符仍拒绝发送信号。
- 日志后端 `/logs` 与 `/kernel/install-log` 共用 Rust 尾读器：以请求开始取得的文件长度为上界，从末尾按 8 KiB 分块回读，足够所需行数即停止，最多读取 256 KiB；仅为最终返回行执行 UTF-8 安全截断（2000 字节上限，超长附省略号）。窗口首个残行丢弃、CRLF、无末尾换行和非法 UTF-8 替换语义保持不变；读取中新增内容留待下次刷新，文件缩短导致读取失败时仍按原有约定返回空列表。默认/上限仍分别为运行日志 80/2000 行、安装日志 40/500 行，安装任务身份和鉴权规则不变。
- 合法 `seq + instance` 任务身份存入 `sessionStorage`。页面重建及 `pageshow` 恢复原任务的轮询，不重复 POST 创建任务；存储不可用时安全降级。不得在无法确认任务时猜测成功或重复提交。
- 语言只在首次初始化时默认中文，不覆盖已有英文。

## 验收边界与当前状态

- G0–G3 按已批准范围完成；真实手机 fnOS App 暂不纳入本轮验收，最终手机端复测仍待用户。fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不表述为已撤销或已修复。
- 官方 PyPI 直连 15 分钟无输出超时；真实阿里云 `qwenpaw==2.2.2.post1` 上游安装/卸载通过，不等于官方 PyPI 直连通过。
- 本文为静态源码契约盘点。候选和历史平台证据见 `docs/session-handoff-2026-11-04.md`、`docs/platform-validation-2026-11-03.md` 与 `docs/unified-gateway-migration-plan.md`；命令存在不代表本轮执行或通过。

项目已有回归入口包括：

- 管理/平台：`python3 scripts/test-platform-callbacks.py`、`python3 scripts/test-management-boundary.py`、`python3 scripts/test-supervisor-api.py`、`bash scripts/test-lifecycle.sh`、`bash scripts/test-config-callback.sh`、`bash scripts/test-venv-recovery.sh`、`python3 scripts/test-runtime-venv-contract.py`。
- 前端：`npm test --prefix web`、`npm run typecheck --prefix web`、`npm run test:e2e --prefix web`、`npm run test:e2e:gateway --prefix web`、`npm run test:e2e:gateway:webkit --prefix web`。
- 门禁：`STRICT_CI=1 bash scripts/lint.sh`、`bash scripts/preflight.sh`、`python3 scripts/gen-audit.py --check`。

`test-supervisor-api.py`、`test-venv-recovery.sh` 等依赖 release supervisor 的场景应先按脚本要求构建并隔离数据目录。CGI 专属测试入口已删除。运行本文件列出的命令不意味着已完成 fnOS 真机、手机 App 或生产发布验收。
