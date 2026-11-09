# 管理层现状契约盘点

- 状态：按 2026-11-09 当前工作树实现更新；本文描述静态源码契约，不代表本轮执行了构建、测试或真机复测。
- 范围：fnOS 生命周期薄入口、Rust supervisor/管理核心、Gateway、私有控制 UDS、受管运行时及 Web 客户端。
- 当前候选为 `build/candidates/1.1.1-11-dual-web-proxy-20261109/qwenpaw-1.1.1-11-x86.fpk`，SHA256 `2db563daa353fca8ac1ed6c59275375787eb1fae37abc722a49434a8098723c1`；supervisor SHA256 `b7813f1ab26315599035acfd042656c82634624c225005c1f0a36d1758bb3f6f`。远端为 `1.1.1-11 running`，内核 `2.3.0b2 running`，无活动事务；不是生产发布。独立TCP反代自动分配、冲突回退、var持久化、前端可改且先bind再切换，当前33083；独立图标无port并走Gateway。双入口真实Chromium鉴权/中文/en保留/无启动超时通过，gzip生效。真实手机复测仍待用户。
- 当前管理 Web 架构为 Gateway-only。G4 已实施：生产 CGI 文件、CGI 子命令、前端 action fallback、CGI 专属测试和 CI 门禁均已删除。下文按当前实现描述，不把历史 CGI 路径写作现状。

## 入口与共享管理核心

| 入口 | 当前契约 |
|---|---|
| `cmd/main` | 固定转发 `service start|stop|restart|status` 到 `qwenpaw-supervisor`；定位与 `exec` 是入口职责，服务身份校验、锁、就绪和停止策略由 Rust 实现。 |
| `cmd/*_callback` | install、upgrade、configure、uninstall 等 fnOS 回调转发固定 `platform` 子命令；向导环境变量由 Rust 校验与处理。平台回调时序/重试语义须按平台证据描述。 |
| `supervisor/src/main.rs`、`cli.rs` | 启动装配共享 `ManagementCore`；提供常驻 `serve`、`service`、`platform`、`doctor --json` 等原生子命令。生产 CGI 子命令 `web api-cgi` / `web index-cgi` 已删除。 |
| `supervisor/src/management.rs` | HTTP、Gateway 与私有平台控制通道共用管理核心，集中操作互斥、恢复门禁、安装提交、运行时健康与初始化等管理规则。 |
| `supervisor/src/platform.rs`、`service.rs` | 分别承接平台生命周期/回调与服务管理；Shell 入口不保留安装、配置、停止或删除策略。 |

## Gateway、HTTP 与 UDS 边界

- fnOS `qwenpaw.console` 与 `qwenpaw.main` 均配置 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock`；主 Web 入口为 `/app/qwenpaw/web/`。两者经同一 Gateway/Socket 路径，不使用 CGI fallback，不裸连 QwenPaw 内核端口，也不使用 `target blank`。
- Gateway UDS 与 `${var_dir}/supervisor-control.sock` 私有控制 UDS 分离。私有 control UDS mode 为 0600，通过 `SO_PEERCRED` 核验 peer UID，只供受信的平台私有控制操作；它不是浏览器或 fnOS Gateway 入口。
- 管理 HTTP loopback 与 Gateway 是不同传输入口，共用管理核心。不能因本机 loopback 或私有 control UDS 请求携带 `X-Trim-*` 就当作 Gateway 身份。
- Rust `web_proxy` 将 `/app/qwenpaw/web/` 下的请求代理到上游 QwenPaw Web，保留上游路径与请求头中的 Bearer Authorization，支持流式 SSE；管理系统自身的 CSRF 校验与上游请求处理相互分离。WebSocket 明确返回 HTTP 501，不能宣称已支持 WebSocket。
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
