# AGENTS.md

## 项目概述

QwenPaw for fnOS 是将 QwenPaw 部署到飞牛 NAS 的轻量外壳包，不是 QwenPaw 本体仓库。外壳负责安装、升级、配置、进程监管和运维控制台；QwenPaw 内核按需从 PyPI 安装。

核心架构主线：

`fnOS 统一网关 → Rust Gateway / Socket → QwenPaw Web`

fnOS 生命周期回调由薄入口转发至 Rust supervisor；管理控制台与 QwenPaw 主 Web 入口均经 Gateway。生产 CGI 已退役。

外壳与内核版本双轨：除用户已明确批准的 `1.3.3-1` 健康 2.3.0 → 2.3.0.post1 事务迁移外，外壳升级不得隐式升级、降级或重装内核。内核版本由版本钉记录，运行环境重建时按版本钉恢复。

## 当前状态与执行顺序

进行后续管理层工作前，先阅读 `docs/session-handoff-2026-11-04.md`、`docs/management-contract-inventory.md` 和 `docs/management-refactor-plan.md`，再核对当前工作树与相关源码。基线包含大量前序未提交实现和文档；必须按当前工作树判断，保留已有改动，不能只依据 HEAD。

**当前本地专项（2026-11-18）：**当前修复候选为 `1.3.3-2`，以官方 `2.3.0.post1`（`ee58cb1a64f2b21e825c525f73dc44bc7941edea`）为内核适配基线，只携带 post1 完整 Console 载荷；健康 2.3.0 在外壳升级时经同一 venv 事务迁移到 post1，失败回滚并保留用户数据。镜像明确缺少 post1 时，仅本次迁移改用官方 PyPI，原镜像设置不变。登录页不再初始化聊天插件，管理日志按 50 行分组跳过屏外绘制。候选 `build/candidates/1.3.3-2-mirror-fallback-20261118/qwenpaw-1.3.3-2-x86.fpk`，33,512,246 字节，SHA256 `a72ab37a62f23a0440ecefa78371d48696d73f212c20437c594cb6ca2446e575`。post1 Console 693 个测试文件、8926 项通过（2 项预期失败），Rust 406+13、静态路由 31、平台 51 项及 FPK 审计通过；真实反代与 WebKit 隔离测量通过。本机已安装 `1.3.3-1` 并迁移到 post1，`1.3.3-2` 尚未部署；真实 iOS 与 24 小时稳定性仍未完成。

**历史性能与重装修复候选（2026-11-17）：**当时 manifest 版本为 `1.2.7-7`；候选 `build/candidates/1.2.7-7-orphan-reinstall-20261117/qwenpaw-1.2.7-7-x86.fpk`，稳定路径 `build/candidates/latest/qwenpaw-latest-x86.fpk`，23,773,553 字节，SHA256 `5218b74df94c2426ecc7e48f5bd57dff66cb86e5d1917d9af7f5b6eed93bf2e7`，包内 musl supervisor SHA256 `9d9ec7bc00f28b18b258dfc2704df2b84aecd7f974b308b2cf3df3a8e45cfcf6`。

Console 的历史候选以官方 v2.3.0 `0a21644e90d8ba37e20035b3bfb63880872cc2a2` 为基线，源码补丁 SHA256 `b0ba1b6e48df72f6c3f7188dbee23a33ce282ef605f2f46401c64356f92d8c2f`，动态预加载并发限制为 16，保留运行时 base-path 和浏览器 Cookie 清理。安装仅接受完整预构建载荷，不再回退下载其他版本源码。历史哈希与 `/proc/exe` 权限回退保持；已登记且与当前目标一致的 supervisor 直接复用，路径变化才停止旧实例；无登记时仅在存在历史安装证据且目标可执行文件一致时接管唯一 orphan，健康内核不因重装被停止，多实例或身份不明仍失败关闭。

历史候选验证：Console 683 个测试文件、8744 项通过（2 项预期失败）；后端相关 52 项、Rust 388+13 项及严格 Clippy 通过；前端 128 项与类型检查通过，Gateway E2E 9 通过、1 跳过；平台回调全量通过。preflight、build/FPK audit 通过，915 个补丁文件哈希一致。首屏 2.921 MiB raw、0.792 MiB Brotli。无旧 PID 登记的隔离重装 8.9 秒完成。远程应用中心已安装 `-7 running`，内核 2.3.0 正常运行且 recovery_required=false；主 Web 聊天输入与设置深链刷新通过，资源/API/页面错误均为 0，资源峰值 18（不写成严格 16）。这是测试候选，不是生产发布。

历史平台证据保留在 `docs/platform-validation-2026-11-03.md` 和 `docs/unified-gateway-migration-plan.md`。历史文档中的旧 FPK 路径和摘要只用于审计，不表示对应本地产物仍然保留或可在当前源码上复现。

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
- `web/`：Preact + TypeScript/TSX 前端源码，esbuild 输出单文件控制台。
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

`STRICT_CI=1` 下不允许因缺少 shellcheck、Node 或 esbuild 而静默跳过门禁。优先使用 `npm ci` 和 Cargo `--locked`，不要无意更新锁文件。

以下测试需要已构建的 release 监督者：

```bash
bash scripts/test-venv-recovery.sh
python3 scripts/test-supervisor-api.py
```

它们应使用临时数据目录和隔离进程，不得操作真实部署。`test-venv-recovery.sh` 是 SIGKILL 崩溃恢复测试，不等同于真实断电验证。

前端还提供 `npm run typecheck --prefix web`、`npm run test:e2e --prefix web`、`npm run test:e2e:gateway --prefix web`、`npm run test:e2e:gateway:webkit --prefix web`；执行前确认依赖与环境就绪。命令存在不代表本轮已运行或通过。

## 版本与修改约定

修改版本时以 `manifest` 为起点，并使用 `scripts/sync-version.py` 同步；用 `python3 scripts/sync-version.py --check` 验证。文件清单变化时同步更新 `AUDIT_REPORT.md`，再通过 `gen-audit.py --check` 验证。

自 2026-11-16 起按用户规则控制版本节奏：每个新功能单独推进一次基础版本，只递增最后一段，例如 `1.2.5 → 1.2.6`；该功能的修复和测试从 `1.2.6-1` 开始，后续依次 `-2`、`-3`。大版本或阶段性更新递增中间段并将末段归零，例如 `1.2.5 → 1.3.0`。不得在同一功能上无意义地连续增加修复号，也不得把多个无关新功能塞进同一版本；历史版本保持原样，不回改。

1.3 性能专项按后续用户补充规则执行：每完成一个阶段推进 `1.3.x`，该阶段修复与测试使用 `-x`；未完成门禁不得记为阶段完成。阶段范围与验收记录见 `docs/performance-governance-1.3.md`。

- 先运行 `git status --short`，保留用户和前序会话的未提交修改；不要用 reset、checkout 或 clean 覆盖它们。
- 遵循现有语言与模块边界：Rust 管系统状态和事务，Shell 管 fnOS 生命周期适配，前端保持 Preact + esbuild，不引入另一套并行管理服务。
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
