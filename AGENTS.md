# AGENTS.md

## 项目概述

QwenPaw for fnOS 是将 QwenPaw 部署到飞牛 NAS 的轻量外壳包，不是 QwenPaw 本体仓库。外壳负责安装、升级、配置、进程监管和运维控制台；QwenPaw 内核按需从 PyPI 安装。

核心架构主线：

`fnOS 生命周期回调 / CGI → Rust supervisor → Python venv 中的 QwenPaw`

外壳与内核版本双轨：外壳升级不得隐式升级、降级或重装内核。内核版本由版本钉记录，运行环境重建时按版本钉恢复。

## 当前整改目标与执行顺序

进行后续管理层工作前，先完整阅读 `docs/session-handoff-2026-11-04.md` 和 `docs/management-refactor-plan.md`，再核对当前工作树与相关源码。基线包含大量前序未提交实现和文档；必须按当前工作树判断，保留已有改动，不能只依据 HEAD。

**当前进度（2026-11-04，初审整改后）：P0–P5 本地实现及安全复核修复已完成；P4 fnOS 统一网关/真机验收仍阻塞。初审指出的崩溃后 service 恢复、Committed 写失败、Web 停止/卸载门禁、UDS instance 归属、TS 非空断言和文档本机路径已整改。最新本地包为 1.1.1 RC1 `build/output/qwenpaw-1.1.1-1-x86.fpk`，SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`，2,071,727 字节；包内 supervisor SHA256 `075ea6f48ab9c617aae9888eea63565e06ee5703e2afcdac5aa307697107c607`。Rust 222+13、Node 30、严格 Clippy/typecheck、lint、audit、自测、API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 与 Chromium 1 均通过；原始 SIGKILL→stop/restart 已在隔离环境实测通过。最新包未另行解包重跑全部包内套件。候选通过本地验证不等于生产可用声明；fnOS 真机网关/鉴权、回调窗口、最低版本、停止预算、真实 PyPI、WebKit/App 和真机回退仍未验证。未 commit、push、发布或部署。本地 `supervisor-control.sock` 不是 fnOS `gatewaySocket`；迁移期 CGI 尚未退役。**

- 目标是让**自研生产管理层**退出 Python 和承载业务的 Shell。QwenPaw 本体只是外部受管程序，其 Python/Node 运行依赖不得因此删除。
- Rust 继续作为唯一系统管理核心；TypeScript 用于前端、客户端协议及合适的工程工具。不新增与 Rust 职责重叠的 Node 管理服务，不另建第二套安装、配置或恢复实现。
- fnOS 必需的 `cmd/*` 是定位原生程序、传递固定子命令并 `exec` 的薄入口；业务判断、配置写入、联网、安装编排和进程清扫统一进入 Rust。
- 构建、打包、CI 和测试驱动仍可保留 Python/Shell，但不得进入生产管理业务链路。
- 受管运行时只能通过集中的 `RuntimeAdapter` 以固定 argv 调用 venv、pip、QwenPaw CLI 及固定探针；不得用 `python -c` 藏 JSON、联网、版本排序或管理策略。
- 保持单 crate 和现有模块主线；HTTP 与私有控制通道共用传输无关管理核心，不做语法级逐行翻译或推倒重写。

阶段计划仍按 `P0 → P1 → P2 → P3 → P4 → P6` 表示发布关键路径；P5（TS 客户端与类型检查）已在本地实现并与主线并行完成。P6 本地候选构建及包内验证已完成，但这不关闭 P4 真机门禁、外部兼容性验证或最终生产发布门禁。回退旧包前必须先完成未决事务恢复，并确认目标旧包兼容当前 schema/layout；真机回退未演练。完整删除用户数据不可回滚。

平台验证继续要求：前置失败、端口不可用、就绪超时和未触发注入点必须使测试失败；健康端点由受管替身自身持有，停止后不得继续健康。测试须记录被测二进制来源与摘要，目录隔离不等于 `/proc` 隔离，发信号前登记并核验目标身份，无关哨兵进程必须存活。

`npm run typecheck --prefix web`、`npm run test:e2e --prefix web` 及原生 CLI/UDS 和集成测试入口当前已存在；执行前仍须确认相关依赖与环境就绪。命令存在不代表本轮已运行或通过。

## 目录与职责

- `manifest`：FPK 版本及应用元数据的单一事实来源。
- `cmd/`：fnOS 生命周期回调仅定位 `qwenpaw-supervisor` 并转发固定 `platform` 子命令；业务逻辑与安全停止由 Rust supervisor 实现。
- `supervisor/`：Rust 常驻监督者，负责内核生命周期、安装事务、配置事务、状态与 HTTP API。
- `supervisor/src/api/`：按状态、生命周期、安装、配置和日志划分的 API 模块。
- `supervisor/src/install/`：安装编排、venv 事务、pip、进程和 I/O；不要恢复成单体 `install.rs`。
- `app/ui/`：仍在使用的迁移期 CGI 入口及桌面配置。`api.cgi` 薄转发到 Rust 原生兼容处理，`index.cgi` 转发到 Rust 静态资源服务；它们不是 fnOS 统一网关鉴权实现，也尚未退役。
- `web/`：Preact + TypeScript/TSX 前端源码，esbuild 输出单文件控制台。
- `wizard/`：安装、升级、配置和卸载向导。
- `config/`：fnOS 权限与资源配置。
- `scripts/`：构建门禁、回归测试、版本同步和审计工具。
- `tools/`：FPK 打包工具。
- `docs/`：设计和交接文档；注意区分历史提案与当前契约。

以下均为生成物或构建目录，不要手工编辑或混入提交：`web/dist/`、`app/www/`、`app/bin/`、`build/`、`supervisor/target/`、`web/node_modules/`。

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

`STRICT_CI=1` 下不允许因缺少 shellcheck、Node 或 esbuild 而静默跳过门禁。优先使用 `npm ci` 和 Cargo `--locked`，不要无意更新锁文件。

以下测试需要已构建的 release 监督者：

```bash
bash scripts/test-venv-recovery.sh
python3 scripts/test-supervisor-api.py
```

它们应使用临时数据目录和隔离进程，不得操作真实部署。`test-venv-recovery.sh` 是 SIGKILL 崩溃恢复测试，不等同于真实断电验证。

修改版本时以 `manifest` 为起点，并使用 `scripts/sync-version.py` 同步；用 `python3 scripts/sync-version.py --check` 验证。文件清单变化时同步更新 `AUDIT_REPORT.md`，再通过 `gen-audit.py --check` 验证。

## 修改约定

- 先运行 `git status --short`，保留用户和前序会话的未提交修改；不要用 reset、checkout 或 clean 覆盖它们。
- 遵循现有语言与模块边界：Rust 管系统状态和事务，Shell 管 fnOS 生命周期适配，前端保持 Preact + esbuild，不引入另一套并行管理服务。
- Rust 修改至少运行对应 `cargo test --locked`；涉及所有 target 或准备交付时运行 Clippy 严格检查。
- Shell/CGI 修改运行 `bash -n` 和综合 lint。现有门禁脚本采用显式收集失败并统一退出的方式，不要盲目加 `set -e` 破坏该模式。
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
- CGI 读操作用 GET、变更用 POST，保留 double-submit cookie CSRF 防护、未知 action 404 和上游 HTTP 状态透传。
- `supervisor-token`、版本钉等敏感或事务文件保持既有权限与原子写语义。
- 打包采用白名单 staging；构建失败不得复用陈旧二进制冒充新产物，FPK 必须通过内容和执行位审计。
- CGI、生命周期入口和构建脚本的可执行位属于发布契约。
- 不未经真机验证就提高最低 fnOS 版本、删除生产 CGI、公开内部管理端口或宣称网关/UDS 路径兼容。

## 文档使用

- `README.md`：用户功能、目录和当前回归入口。
- `docs/backend-design.md`：历史提案，不作为当前实现依据。
- `docs/management-refactor-plan.md` 与 `docs/management-contract-inventory.md`：管理层重构时先阅读的当前计划与契约。
- `docs/session-handoff-2026-11-04.md`：最新重构交接；以其中记录的候选摘要、实际通过项和外部未验证边界为准。
- `docs/platform-validation-2026-11-03.md`：平台实测证据与未验证项。

交付时明确列出实际运行的检查、候选产物（如已构建）以及真机、网络或兼容性未验证项。不要依据旧文档或单次单元测试宣称整个 FPK 已在 fnOS 生产环境验证。
