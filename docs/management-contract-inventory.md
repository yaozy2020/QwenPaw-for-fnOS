# 管理层现状契约盘点

- 状态：按当前工作树实现更新；本文件描述静态源码契约，不代表本轮执行了构建、测试、回调或平台验收。
- 范围：fnOS 生命周期入口、Rust supervisor/管理核心、HTTP 与 CGI 兼容层、私有控制 UDS、受管运行时及 Web 客户端。
- 总体进度：P0–P5 的本地实现已完成；P4 的 fnOS 统一网关与真机验证仍阻塞；P6 最终摘要待完成。平台验证边界见 `platform-validation-2026-11-03.md`。

## 入口与共享管理核心

| 入口 | 当前契约 |
|---|---|
| `cmd/main` | 固定转发 `service start|stop|restart|status` 到 `qwenpaw-supervisor`；定位与 `exec` 是入口职责，服务身份校验、锁、就绪和停止策略由 Rust 实现。 |
| `cmd/*_callback` | install、upgrade、configure、uninstall 等 fnOS 回调转发固定 `platform` 子命令；向导环境变量在 Rust 校验与处理。回调执行窗口及平台实际重试语义尚未在 fnOS 真机验证。 |
| `supervisor/src/main.rs`、`cli.rs` | 启动装配共享 `ManagementCore`；支持常驻 `serve`、`service`、`platform`、`doctor --json`、`web api-cgi` 与 `web index-cgi` 原生子命令。具体操作命令及输出以 CLI 源码为准。 |
| `supervisor/src/management.rs` | HTTP 和私有平台控制通道共用管理核心，集中操作互斥、恢复门禁、安装提交、运行时健康与初始化等管理规则。 |
| `supervisor/src/platform.rs`、`service.rs` | 分别承接平台生命周期/回调与服务管理；Shell 入口不保留安装、配置、停止或删除策略。 |

## HTTP、CGI 与 UDS 边界

- 管理 HTTP 监听 `127.0.0.1:${SUPERVISOR_PORT}`（默认 19090），不是对外公开端口，也不是 fnOS 网关服务。HTTP handler 通过监督者 token 校验请求，路由状态码和副作用以 `supervisor/src/api/` 当前实现为准。
- `supervisor/src/control.rs` 创建 `${var_dir}/supervisor-control.sock`，Socket 权限为 0600，并通过 `SO_PEERCRED` 核验 peer UID。它用于受信的平台私有控制操作（如配置、镜像、安装/状态及运行时 health/init），并与 loopback HTTP 共用管理核心。
- 该私有 UDS **不是** fnOS 的 `gatewaySocket`。没有据此实现或证明统一网关路由、网关 Header 身份、fnOS 登录态、管理员角色授权、CSRF 网关链路或最低系统版本兼容。
- `app/ui/api.cgi` 是薄 Shell `exec` 入口；`supervisor/src/api_cgi.rs` 处理兼容 action、方法/CSRF 校验和 loopback HTTP token 转发。镜像写入等私有平台操作经控制 UDS。它是迁移期兼容桥，仍随包使用，不是网关认证实现，也未退役。
- `app/ui/index.cgi` 是薄 Rust `web index-cgi` 转发入口；`supervisor/src/index_cgi.rs` 负责静态资源服务。当前静态 CGI 仍在打包/使用。
- `web/src/api.ts` 仍使用 api.cgi 兼容路径；前端 API 客户端、任务归属逻辑与后端契约分层，尚未迁到经真机验证的网关前缀。

## 配置、安装与运行时

- 配置入口统一进入 Rust 平台模块及 `ManagementCore`。`settings_transaction.rs` 对端口、日志级别及桌面入口维护可恢复事务；状态查询不应改写配置。镜像由 Rust 控制面持久化，不能恢复 CGI 直接写文件的旧职责。
- 安装编排和持久化环境事务位于 `supervisor/src/install/`。安装使用独立 staging/rollback venv；健康检查、版本校验及提交决定遵循事务状态；未提交失败先停止新实例、恢复环境与 pin，再恢复旧服务。恢复不完整时保留证据并阻断写操作。
- `supervisor/src/runtime.rs` 的 `RuntimeAdapter` 通过固定 argv 调用受管 Python/venv、pip、QwenPaw CLI，以及限定的 metadata/import 探针和 init。管理策略、JSON 业务、联网版本解析不再由 Python 实现；QwenPaw 自身的 Python/Node 依赖仍是受管运行时要求。
- `VenvTransaction::activate` 在激活暂存环境前，由 `supervisor/src/install/io.rs` 的 `relocate_console_script_shebangs` 将匹配的 console-script shebang 调整为活动 venv 路径。`scripts/test-runtime-venv-contract.py` 提供真实 venv 的 `sys.prefix`、import、shebang/执行及后续 pip 契约检查入口；本文件不宣称本轮执行了该检查。
- `supervisor/src/version_index.rs` 使用 Rust HTTP 与 HTML 解析实现 Simple Index 版本查询，不再是内嵌 Python urllib/HTMLParser。TLS、网络可用性与 fnOS 实际镜像访问仍须按相应测试/平台环境验证。
- `cmd/lib.sh` 与 `cmd/supervisor_ctl.sh` 已删除；`cmd/main` 和平台回调为固定 Rust 子命令的薄转发入口。

## 回归入口与证据边界

仓库已有以下本地入口（存在不代表本轮执行或通过）：

- 平台回调与传输：`python3 scripts/test-platform-callbacks.py`、`python3 scripts/test-native-api-cgi.py`、`python3 scripts/test-native-index-cgi.py`。
- 管理/架构：`python3 scripts/test-management-boundary.py`、`python3 scripts/test-supervisor-api.py`、`bash scripts/test-lifecycle.sh`、`bash scripts/test-config-callback.sh`、`bash scripts/test-venv-recovery.sh`。
- venv 契约：`python3 scripts/test-runtime-venv-contract.py`。
- 前端：`npm test --prefix web`、`npm run typecheck --prefix web`、`npm run test:e2e --prefix web`；浏览器 E2E 位于 `web/e2e/control-panel.spec.ts`。
- 门禁/审计：`STRICT_CI=1 bash scripts/lint.sh`、`bash scripts/preflight.sh`、`python3 scripts/gen-audit.py --check`。`lint.sh` 与 management-boundary 属实现/架构门禁，不是文档一致性检查。

`test-supervisor-api.py`、`test-venv-recovery.sh` 等依赖 release supervisor 的场景应先按脚本要求构建并隔离数据目录。交接材料报告过 Chromium E2E 1 项、Node 测试 28 项及严格 typecheck 通过；这些是此前提供的交接结果，本轮没有重跑，不能写作本轮新验证。

## 尚未由本地实现证明的事项

- fnOS 统一网关 Socket 接入、网关路径/前缀、请求身份上下文、Header 覆盖、登录及管理员授权、CSRF 和移动 App 行为。
- manifest 声明的最低 fnOS 版本与当前目标版本兼容性，应用中心的回调顺序、升级/卸载执行窗口及桌面入口动态注册行为。
- 最终候选 FPK 的打包物摘要、包内二进制与已测二进制一致性、真机安装/升级/重启/回退/卸载验收。

本地私有 control UDS、loopback API、原生 CGI 和测试替身都不能代替以上平台证据；未完成真机验证前，不宣称 CGI 已退役、网关路径已验收或发布门禁已全部关闭。
