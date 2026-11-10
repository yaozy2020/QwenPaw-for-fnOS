# AGENTS.md

## 项目概述

QwenPaw for fnOS 是将 QwenPaw 部署到飞牛 NAS 的轻量外壳包，不是 QwenPaw 本体仓库。外壳负责安装、升级、配置、进程监管和运维控制台；QwenPaw 内核按需从 PyPI 安装。

核心架构主线：

`fnOS 统一网关 → Rust Gateway / Socket → QwenPaw Web`

fnOS 生命周期回调仍由薄入口转发至 Rust supervisor；管理控制台与 QwenPaw 主 Web 入口均经 Gateway。生产 CGI 已退役。

外壳与内核版本双轨：外壳升级不得隐式升级、降级或重装内核。内核版本由版本钉记录，运行环境重建时按版本钉恢复。

## 当前整改目标与执行顺序

进行后续管理层工作前，先完整阅读 `docs/session-handoff-2026-11-04.md` 和 `docs/management-refactor-plan.md`，再核对当前工作树与相关源码。基线包含大量前序未提交实现和文档；必须按当前工作树判断，保留已有改动，不能只依据 HEAD。

**当前状态（2026-11-10）：**当前 RC 候选为 1.1.2-9：`build/candidates/1.1.2-9-rc1-20261110/qwenpaw-1.1.2-9-x86.fpk`，FPK SHA256 `8729939ab0f3adb8ee68575f26a32042a07abb3848d1812b911137868565e95a`，musl 静态 supervisor SHA256 `1c56b64e76db11bb87ddc6cb5b1a2f817d0107a2347486adef6ed37d5ace1a21`。此前官方升级、主 Web、真实 uv 重装、管理员登录/重置均有证据；1.1.2-7 已验证匿名配置 401、真实 Bearer 配置访问和 public 同端口切换。用户明确边界：UNINSTALL+keep/unknown 必须删除 venv；仅官方 `TRIM_APP_STATUS=UPGRADE` 保留，remove 与仅卸载内核不变。真实更新顺序测试证明同 inode 复用且不调用安装器。Rust 317+13、Clippy/rustfmt、Node 41/typecheck、FPK audit 通过。远端已通过官方 7→8 和 8→9 两阶段单次升级验收；两次均未观察到 venv 删除、staging 或 pip 写入，8→9 前后 venv/Python inode、版本钉及 pip 日志摘要不变，证据位于 `/tmp/qwenpaw-1129-official-20261110/`。7→8 的迁移结果与 8→9 的免重建结论分别记录；当前远端 1.1.2-9 running、内核 2.3.0b2 健康。原生 restart 未有当前候选证据。未 commit、push 或发布；RC1 发布资产不得与本地候选混用。

**历史基线（2026-11-04，G1 前旧包）：P0–P5 本地实现及安全复核修复已完成；P4 fnOS 统一网关/真机验收仍阻塞。初审指出的崩溃后 service 恢复、Committed 写失败、Web 停止/卸载门禁、UDS instance 归属、TS 非空断言和文档本机路径已整改。当时本地包为 1.1.1 RC1 `build/output/qwenpaw-1.1.1-1-x86.fpk`，SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`，2,071,727 字节；包内 supervisor SHA256 `075ea6f48ab9c617aae9888eea63565e06ee5703e2afcdac5aa307697107c607`。Rust 222+13、Node 30、严格 Clippy/typecheck、lint、audit、自测、API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 与 Chromium 1 均在该历史基线上通过；原始 SIGKILL→stop/restart 已在隔离环境实测通过。该旧包未另行解包重跑全部包内套件，不代表当前 G1 源码已打包。候选通过本地验证不等于生产可用声明。未 commit、push、发布或部署。**

**历史统一网关/真机进度（截至 2026-11-08；当前状态见文档本轮 2026-11-09 更新）：G0/G1 已完成；G2 本地实现、真实 Chromium 双协议回归和候选包内复验已完成；G3 的 helper-first 修复与 1.1.1-2 → 1.1.1-3 正式外壳升级、A/B/C/D 远端验收均已通过：A 完整 GET/HEAD/query/404/编码拒绝/Content-Type/SSE 断连，B 官方连续 stop/start/start/reconnect，C 真实 `app.sock` stale/active listener/普通文件/软链接/unknown owner 的 fail-closed 与恢复，D 阿里云真实 `qwenpaw==2.2.2.post1` 安装/健康/stop/start/stop、仅卸载以及新包 manager SIGKILL→官方 start 接管原内核并停止卸载。匿名/伪造会话仅返回平台 HTTP 200 `text/plain` `invalid token`，不构成公开 HTML 或授权通过。用户明确暂时排除真实手机 fnOS App，不纳入本轮 G3 验收、不作为当前 G3 阻塞；本地 WebKit/Chromium Gateway 2/2 保留为浏览器兼容证据。G3 不能因 App 排除自动整体关闭：真实失效会话门禁仍未通过；官方 PyPI 直连 15 分钟无输出超时且未重投，完整生产路径/错误矩阵和发布门禁未完成；G4 未开始，CGI 仍保留，不构成生产发布结论。** 截至 2026-11-08，候选桌面入口已切到 `/app/qwenpaw/`，前端使用 `/app/qwenpaw/api`，`cmd/main` 固定启用 `${TRIM_APPDEST}/app.sock`；CGI 当时仅作候选迁移回退物料保留。当时测试候选为 `build/candidates/1.1.1-3-gateway-g3-20261108/qwenpaw-1.1.1-3-x86.fpk`，SHA256 `94679e4d8343c1bf341e8a1cc2a1451a9ad0ad3296d2fc668deab456db601b51`；supervisor SHA256 `2829f1a4079139f166f78478de804d3f619226d646ea93d58d0965718dbc4400`。该候选已从 1.1.1-2 经官方 update 单次升级并在远端验收；1.1.1-2 c571 为历史本地候选，667 为更早远端包。证据总目录为 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`。**以上均为历史记录；截至 2026-11-09，生产 CGI 已退役、当前架构为 Gateway-only，当前 1.1.1-6 仅为测试候选而非生产发布；具体状态见本节顶部。**

本地最终修复/验证记录为 `test-platform-callbacks` 33 项、卸载 Rust 15 项、严格 Clippy、preflight、`STRICT_CI`、build、audit 均通过。新增 `supervisor/src/install/process.rs` 短命子进程登记竞态修复：仅同一身份已退出时允许 executable 缺失，存活或身份变化仍失败关闭；RED/GREEN 证据为 `/tmp/qwenpaw-g3-process-identity-20261107/`，install 87、runtime 5、严格 Clippy、user namespace 全 crate 276+13 通过。历史 `905e`/`59023c` 结果及一次性 sibling 改名仅是限制或基线准备，不计为正式升级通过；最终回调契约以 helper-first 取代旧 sibling-first 表述。

**2026-11-08补充验证：**本地 WebKit engine 选择已加入 `web/playwright.config.ts`、`web/e2e/run.mjs`、`web/package.json`、`web/e2e/README.md`；隔离 Docker WebKit Gateway 2/2、Chromium Gateway 2/2 均通过，日志为 `/tmp/qwenpaw-gateway-webkit-20261107/e2e-webkit-https.log`，本地 WebKit/Chromium 继续作为浏览器兼容证据；真实手机 fnOS App 暂不纳入本轮 G3。WebKit 丢回执不是生产 beacon 失败：第三次 beacon 返回 `success=false`，复用同一 `seq/instance`，未新建事务；测试严格检查 `done`、`upgrading=false`、`recovery=false` 及 marker/pin/健康，不依赖可选 pip 日志，跨引擎 console 错误分别精确匹配。此前本地 process identity 修复与回归保持通过。1.1.1-3 远端 A/B/C/D 验收及最终卸载/重装证据统一位于 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`；关键记录包括 `matrix-c-remaining.json`、`matrix-d-orphan-followup.json`、`matrix-http-auth-gate.json`、`final-cleanup-verification-20261108.json`。阿里云真实 `qwenpaw==2.2.2.post1` 冒烟通过；官方 PyPI 直连 15 分钟无输出超时且未重投。

用户已明确批准最低 `os_min_version=1.2.0800`，manifest 已更新；低于此版本的旧系统不再承诺 CGI 兼容支持，不另行维护旧系统 CGI 兼容发布路径。G0 隔离 fnOS 探针确认完整 `/app/qwenpaw` 前缀保留、客户端伪造 `X-Trim-*` 被真实会话值覆盖，peer 为 `/usr/trim/bin/trim_http_cgi`（UID `0` / GID `0`）；卸载回调时 target binary 已缺失，但 `TRIM_PKGVAR` 中的 helper 仍可执行。本地和最终远端回调查找顺序均为先检查可执行的 `${TRIM_PKGVAR}/qwenpaw-supervisor-helper`，再检查 sibling，最后 fail closed `127`；install/upgrade-init 在 target 可执行窗口原子生成普通文件 `0750` helper，upgrade/uninstall 可借此处理 target binary 缺失时的回调，install 不使用该 fallback。此前 1.1.1-2/667 远端包的 keep/回退路径证据保留为历史；本次 1.1.1-3 remove 一次删除 target、appdata、appconf、apphome、shared、auth 测试数据及 helper/token/socket，范围外 sentinel 和其他应用不变，随后同一 FPK 官方重装一次恢复。最终远端为 App Center `1.1.1-3 running`、唯一 manager、Gateway HTTP 200、无 QwenPaw 内核/未决事务/18088 listener，正确 gateway/control UDS 均 mode `0600`、UID/GID `994/989`。匿名/伪造会话的页面/API/SSE 均为平台 HTTP 200 `text/plain` `invalid token`，不构成公开 HTML 或授权通过；用户暂时排除真实手机 fnOS App，不纳入本轮 G3 验收。新 1.1.1-3 Playwright Chromium UI logout 证据（`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`）显示“我的账号→退出登录”导航至 `/login`，随后 UI 重新登录成功；未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE。该结果是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是 QwenPaw 应用层鉴权绕过，也不是自然时间过期测试；真实失效会话门禁仍未通过。原始 remove 脚本曾误读字段/未重算 SHA，综合及最终独立证据为准，原始记录不隐藏。以上 G3/G4 与 1.1.1-3 状态为截至 2026-11-08 的历史记录；截至 2026-11-09，G3 已按批准范围完成，G4 已实施并退役生产 CGI。当前候选和远端状态见本节顶部；历史平台证据及矩阵见 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`、`docs/platform-validation-2026-11-03.md` 和 `docs/unified-gateway-migration-plan.md`。

- 目标是让**自研生产管理层**退出 Python 和承载业务的 Shell。QwenPaw 本体只是外部受管程序，其 Python/Node 运行依赖不得因此删除。
- Rust 继续作为唯一系统管理核心；TypeScript 用于前端、客户端协议及合适的工程工具。不新增与 Rust 职责重叠的 Node 管理服务，不另建第二套安装、配置或恢复实现。
- fnOS 必需的 `cmd/*` 是定位原生程序、传递固定子命令并 `exec` 的薄入口；业务判断、配置写入、联网、安装编排和进程清扫统一进入 Rust。
- 构建、打包、CI 和测试驱动仍可保留 Python/Shell，但不得进入生产管理业务链路。
- 受管运行时只能通过集中的 `RuntimeAdapter` 以固定 argv 调用 venv、pip、QwenPaw CLI 及固定探针；不得用 `python -c` 藏 JSON、联网、版本排序或管理策略。
- 保持单 crate 和现有模块主线；HTTP 与私有控制通道共用传输无关管理核心，不做语法级逐行翻译或推倒重写。

阶段计划仍按 `P0 → P1 → P2 → P3 → P4 → P6` 表示发布关键路径；P5（TS 客户端与类型检查）与主线并行。管理层主线与 G0–G4 的已批准范围已完成：生产管理 Web 为 Gateway-only，CGI 已退役。当前候选与验证边界以本节顶部 2026-11-10 状态为准；最终远端验收完成前不得宣称真机通过或生产发布。fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不称会话已撤销或问题已修复。官方 PyPI 直连 15 分钟无输出超时；阿里云真实 `qwenpaw==2.2.2.post1` 安装/卸载通过，不等于官方 PyPI 直连通过。完整删除用户数据不可回滚。

平台验证继续要求：前置失败、端口不可用、就绪超时和未触发注入点必须使测试失败；健康端点由受管替身自身持有，停止后不得继续健康。测试须记录被测二进制来源与摘要，目录隔离不等于 `/proc` 隔离，发信号前登记并核验目标身份，无关哨兵进程必须存活。

`npm run typecheck --prefix web`、`npm run test:e2e --prefix web`、`npm run test:e2e:gateway --prefix web`、`npm run test:e2e:gateway:webkit --prefix web` 及原生 Gateway UDS/集成测试入口当前已存在；执行前仍须确认相关依赖与环境就绪。CGI 专属测试入口已删除。命令存在不代表本轮已运行或通过。

## 目录与职责

- `manifest`：FPK 版本及应用元数据的单一事实来源。
- `cmd/`：fnOS 生命周期回调仅定位 `qwenpaw-supervisor` 并转发固定 `platform` 子命令；业务逻辑与安全停止由 Rust supervisor 实现。
- `supervisor/`：Rust 常驻监督者，负责内核生命周期、安装事务、配置事务、状态与 HTTP API。
- `supervisor/src/api/`：按状态、生命周期、安装、配置和日志划分的 API 模块。
- `supervisor/src/install/`：安装编排、venv 事务、pip、进程和 I/O；不要恢复成单体 `install.rs`。
- `app/ui/`：Gateway 桌面入口配置与图标；生产 CGI 已退役，管理控制台和 `qwenpaw.main` 共用 Gateway/Socket。
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
- Gateway 管理写操作须保留 CSRF 防护、方法约束和未知路由 404；上游代理请求不得错误套用管理系统 CSRF，须按当前代理契约透传所需授权与响应。
- `supervisor-token`、版本钉等敏感或事务文件保持既有权限与原子写语义。
- 打包采用白名单 staging；构建失败不得复用陈旧二进制冒充新产物，FPK 必须通过内容和执行位审计。
- fnOS 生命周期入口、Gateway 配置和构建脚本的可执行位属于发布契约；生产 CGI 已退役，不恢复 CGI/action 隐式回退。
- 不未经真机验证就提高最低 fnOS 版本、公开内部管理端口或宣称网关/UDS 路径兼容。

## 文档使用

- `README.md`：用户功能、目录和当前回归入口。
- `docs/backend-design.md`：历史提案，不作为当前实现依据。
- `docs/management-refactor-plan.md` 与 `docs/management-contract-inventory.md`：管理层重构时先阅读的当前计划与契约。
- `docs/unified-gateway-migration-plan.md`：Gateway-only 当前架构、历史 G0–G4 迁移记录、平台证据边界及回退纪律；G3 按已批准范围完成，G4 已实施并退役生产 CGI。当前测试候选及远端状态见文档顶部；手机 App 最终复测仍待用户，fnOS logout 后旧会话有效是已知且不阻塞应用验收的平台问题；不得把候选或远端 `running` 写成生产发布。
- `docs/session-handoff-2026-11-04.md`：本地候选与重构交接；其中候选摘要和本地通过项继续有效，平台/最低版本状态以随后更新的验证记录及网关迁移计划为准。
- `docs/platform-validation-2026-11-03.md`：平台实测证据、用户批准的最低版本和剩余未验证项。

交付时明确列出实际运行的检查、候选产物（如已构建）以及真机、网络或兼容性未验证项。不要依据旧文档或单次单元测试宣称整个 FPK 已在 fnOS 生产环境验证。
