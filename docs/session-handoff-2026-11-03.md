# 新会话交接：QwenPaw-for-fnOS 管理层整改

> 这是一份供新会话读取并继续实施的交接指令，不是 Shell 可执行文件。
> 交接时间：2026-11-03。当前会话只新增整改计划和本交接文件，没有开始本轮去脚本化实现。

## 一、用户目标及已确定边界

项目：仓库根目录。

仓库：`https://github.com/yaozy2020/QwenPaw-for-fnOS`

参考项目：`https://github.com/shuangji66/dsh`

用户已经澄清：

1. 退出 Python 和 Shell，指的是**我们自研的生产管理层**。
2. **QwenPaw 本体不在重构规划内**，只作为外部受管程序。
3. **TypeScript 可以使用**，不是必须全 Rust。
4. 保留现有主线，继续模块化、事务化，不推倒重写。
5. 已要求详细整改计划和测试方案，现准备在新会话继续完成实施。

当前推荐并已写入计划的实现方向：

- Rust 保留系统管理核心；TS 保留前端、客户端协议，并可用于适合的测试/工程工具。
- 不因参考项目使用 Go 就换语言，不新增与 Rust 职责重叠的 Node 管理服务。
- 安装、修复、版本、镜像、配置、生命周期、鉴权、任务、恢复统一进入 Rust 管理核心。
- fnOS 强制要求的 `cmd/*` 可以保留薄脚本，只定位并 exec 原生程序，不保留业务。
- 工程构建、打包、CI、测试驱动可以继续使用 Python/Shell；不能误解为整个仓库必须删除所有脚本。
- Rust 可通过集中 RuntimeAdapter 调用 Python venv/pip/QwenPaw CLI 和固定 metadata/import 探针。这属于外部运行时适配，不能把联网、JSON、版本排序和管理策略藏进 Python。
- 不擅自修改 QwenPaw 内部数据格式，不将程序回滚等同于用户数据回滚。

## 二、先读什么

完整可执行计划：

`docs/management-refactor-plan.md`

该文件包含：现状证据、目标架构、CLI/锁/任务契约、事务边界、P0—P6实施阶段、分层测试、24项用例、故障矩阵、发布及回退标准。

优先读：

1. 本交接文件。
2. `docs/management-refactor-plan.md` 全文。
3. 适用的项目指令文件；前一轮在项目内没有找到 AGENTS.md，但新会话仍以实际环境为准。
4. 当前 `git status --short`，核实既有改动，不能用 reset/checkout/clean 清掉它们。
5. 针对当前阶段读取实际代码；无需重新广泛调查 DSH，也不要重复历史审查全过程。

`docs/backend-design.md` 已明确标为历史提案，里面有早期 Python/FastAPI 和已变更决策，**不能作为当前实现依据**。

## 三、当前工作树：必须保留已有改动

调查基准 HEAD：

`4f961d0f2bfc37635681f94e50fbb764995be6f5`

注意：**当前工作区包含前序会话已实现且未提交的模块化/事务化改动，单独 checkout HEAD 无法得到现状。**

交接前状态如下；本交接文件是在该次 status 后新增：

```text
 M .github/workflows/ci.yml
 M .github/workflows/release.yml
 M README.md
 M cmd/config_callback
 M cmd/main
 M scripts/lint.sh
 M scripts/test-lifecycle.sh
 D supervisor/src/api.rs
 D supervisor/src/install.rs
 M supervisor/src/kernel.rs
 M supervisor/src/main.rs
 M web/package.json
 M web/src/main.ts
?? docs/management-refactor-plan.md
?? scripts/test-config-callback.sh
?? scripts/test-supervisor-api.py
?? supervisor/src/api/
?? supervisor/src/desktop.rs
?? supervisor/src/install/
?? supervisor/src/runtime_settings.rs
?? supervisor/src/settings_transaction.rs
?? web/src/installTask.mjs
?? web/test/
```

`api.rs/install.rs` 的删除是拆分为目录模块的结果，不是意外丢文件。

不要操作同级项目 `project`，那是无关项目。

## 四、前序已经实现的工作

这些是现有基础，应复用而不是重新实现：

- `supervisor/src/api/` 已按状态、生命周期、安装、配置、日志分组。
- `supervisor/src/install/` 已拆分编排、事务、进程、pip、I/O、渠道及测试。
- 新增 `runtime_settings.rs`、`settings_transaction.rs`：端口/日志级别/桌面配置快照、提交/回滚、启动恢复、恢复失败门禁。
- `desktop.rs` 负责原子、定点修改桌面入口；状态查询不再写 UI 配置。
- 安装 worker 的生命周期、shutdown latch、恢复门禁、唯一终态和安全收尾 busy 规则已加强。
- 认证恢复失败会阻止继续启动；关闭过程中禁止新内核启动。
- `cmd/config_callback` 已变为监督者 `/settings` 客户端，不再失败后自行写配置，但它仍是 Bash+curl+Python，需要本轮原生化。
- `cmd/main` 已修复锁 FD 继承和取锁失败仍继续执行的问题，需要迁移时保留行为。
- 前端 `installTask.mjs` 严格按 `seq + instance` 判断任务归属。
- 增加配置回调和真实监督者 HTTP 集成测试，并接入现有 CI/Release。

但**尚未做到管理层去脚本化**。安装/升级 Shell、镜像 CGI 写入及管理用 Python 仍存在。

## 五、既有验证结果：历史证据，不是本轮新验证

在更早的实现轮次，以下检查曾通过：

- `cargo test --locked`：80 个测试通过。
- `cargo clippy --locked --all-targets -- -D warnings`：通过。
- preflight、gen-audit --check、STRICT_CI lint：通过。
- 前端 Node 纯逻辑测试：21 个通过。
- `bash scripts/test-venv-recovery.sh`：七个 SIGKILL 命名点通过当时断言。
- `python3 scripts/test-supervisor-api.py`：四组 HTTP/恢复/关闭测试通过。
- 构建、FPK审计及摘要校验通过；当时包内 supervisor/cmd/CGI 与被测文件匹配。

历史候选产物：

`build/output/qwenpaw-2.1.0-9-x86.fpk`

历史 SHA256：

`ba16f40b987fbe8a795fe584747396d0af0b06801fa8749846a8dea331bde623`

该包是**前序事务化改造的候选**，不是本轮管理层去脚本化产物。若要使用，先验证文件和摘要，不能假定它仍未变化。

此前没有完成真实 fnOS GUI、统一网关、真实 PyPI 下载/上游运行冒烟。本轮计划调查未运行任何新测试。

计划调查发现现有测试的覆盖限制，必须补强；不要把“旧测试通过”扩张成全部架构已经验证。

## 六、已经核实的重要问题及入口

以下行号为交接时源码，实施后可能变化：

| 位置 | 当前事实 |
| --- | --- |
| `cmd/install_callback:153` | Shell 自己 pip 安装、写 pin、初始化 |
| `cmd/upgrade_callback:29` | Shell 删除坏 venv、重建并按 pin 恢复 |
| `cmd/lib.sh` | 重复路径、镜像、pin、init和桌面配置策略 |
| `cmd/main`、`cmd/supervisor_ctl.sh` | 管理器启停、PID/starttime身份和锁仍由Shell实现 |
| `cmd/uninstall_callback` | 数据保留/删除、进程停止仍由Shell编排 |
| `cmd/config_callback:114/160` | Python构造/解析JSON；curl提交设置 |
| `app/ui/api.cgi:174` | 直接写 `pip-mirror`，绕过配置核心 |
| `app/ui/api.cgi:254/308` | 版本响应处理、JSON校验仍用Python |
| `supervisor/src/api/installation.rs:130` | 内嵌Python urllib/HTMLParser查索引 |
| `supervisor/src/runtime_settings.rs:2` | 配置业务依赖API AppState，需解耦传输 |
| `supervisor/src/main.rs:330` | 当前仅回环TCP监听，无网关UDS |
| `web/src/main.ts:5` | API地址绑定CGI |
| `web/package.json` | 只有esbuild构建，没有真正的TS typecheck |
| `build.sh:143`、`scripts/audit-fpk.sh:90`、CI`:70` | 打包/审计/CI强制要求CGI，需要同步整改 |

`manifest` 当前声明 x86、最低 fnOS 1.0.0，以及 `nodejs_v24:python312` 依赖。不要因为管理层退出Python就删除受管程序运行依赖；Node也在内核PATH里，不能当成纯前端构建依赖随意移除。

## 七、实施的关键约束

### 7.1 不能只迁移语法

HTTP、平台CLI和启动恢复必须共用管理核心。不能把旧Shell逐行翻译成另外一套Rust安装器，也不能拆成TS决策、Rust半执行、脚本兜底。

### 7.2 在线/离线所有权

- 常驻服务持有 `supervisor.lock`；离线执行只有取到同一所有权锁才能写。
- 平台生命周期锁串行化start/stop/restart/升级/卸载交接。
- 服务持锁但IPC失败时必须报告异常，不能离线双写。
- 锁FD不继承给后台服务/受管进程，明确锁顺序和有界等待。
- 状态查询无副作用；恢复只能由独占恢复入口执行。

### 7.3 平台可行性要先核实

- fnOS网关目标：稳定`/app/qwenpaw`前缀、gatewaySocket/UDS。
- 真机核对网关用户头覆盖、管理员授权、CSRF、Socket权限、前缀、移动端、HTTPS及系统兼容范围。
- 文档有接口不等于最低fnOS 1.0.0已验证支持。
- install_init时程序是否存在、upgrade各阶段路径、uninstall_callback时target是否被删必须实测。
- 卸载原生程序不可用时不能偷偷保留Shell删除业务；按计划验证原生执行窗口/helper方案。
- 未通过平台门禁前不删除生产CGI、不公开内部管理端口、不默默提高最低系统版本。
- 缺真机条件时明确阻塞项，继续可独立推进的核心和隔离测试，不伪造结果。

### 7.4 持久化兼容与事务

- 第一轮保留现有配置文件和venv布局，不捆绑全新存储格式。
- 支持当前schema=1旧journal；未知/损坏/路径越界则保留证据、阻断变更、允许诊断。
- 旧桌面快照跨升级恢复后，不能把新网关入口覆盖回旧CGI。
- 已持久化提交决定后以完成提交为主；未提交恢复旧代。恢复失败不能报告成功。
- 完整删除用户数据不可逆，不声称可事务回滚。
- 真实venv staged→active后的console-script shebang、sys.prefix、import、pip必须验证；假解释器不足以证明目录改名安全。

## 八、先补强的测试

1. `scripts/test-venv-recovery.sh:61` 找不到端口只return；调用处未统一检查，最后只看FAIL。应让前置失败不能打印全通过。
2. 同脚本`:106`的健康server是独立进程、固定返回新版本。改为受管替身自己持有HTTP端口，停止后就不健康。
3. `supervisor/src/kernel.rs:447`并发测试未创建解释器，spawn可一直走未安装分支。补真实辅助进程启动与独立CLI多进程竞争。
4. HTTP集成增加全部路由鉴权负例、退出码、全部后代清理和无关进程存活断言。
5. 目录隔离不等于/proc隔离。测试发信号必须核对目标身份，不能误伤当前真实部署。
6. 引入原生Rust测试辅助进程模拟pip/metadata/内核健康/进程树，测试驱动可以继续使用工程Python/Shell。
7. 测试记录被测binary摘要；最终用FPK解出的程序重跑核心黑盒用例。
8. SIGKILL是进程崩溃测试，不是供电丢失验证。

详细24项用例和故障矩阵直接按计划执行，不要重新设计另一份不一致清单。

## 九、推荐推进顺序

`P0契约/平台/测试可信度 → P1管理核心和锁 → P2配置/镜像/生命周期 → P3安装/修复/卸载 → P4网关/静态服务/版本查询 → P6交付`

P5的TS客户端/typecheck/浏览器测试在协议稳定后穿插推进；浏览器真实网关验收依赖P4。

新会话的第一个可交付切片：

1. 读取计划和当前状态，核实平台环境能否完成P0实验。
2. 修补可独立复现的测试失败传播和替身健康归属问题。
3. 冻结现有命令/文件/状态码契约，再开始提取传输无关管理核心。
4. 每个阶段跑对应检查并更新实施进度，避免一次改完所有入口后才测试。

不要再次停留于“可以做/建议做”。用户在新会话粘贴执行指令后，应继续实现、测试和交付；遇到真机/权限/兼容限制要如实记录并推进其余独立任务。

## 十、工具、权限与交付纪律

- 当前无在途后台任务。前一会话的只读调查已结束，不必寻找或等待旧agent。
- 不自动commit/push/tag/发布/部署，不执行真实卸载或删除数据。以上需要当前会话明确授权。
- 不清理或覆盖已有未提交改动，不改无关项目。
- 测试默认临时TRIM根和隔离进程；外部故障演练只能在专用测试环境。
- 用户不喜欢重复确认。已明确范围内的可逆代码修改与测试直接推进，不因普通实现选择停下来问。
- 不假定旧会话的技能已加载或权限模式仍相同；遵循新会话实际指令。是否使用子agent按新会话授权规则决定。
- 中文沟通，更新要说明发现、完成项、下一步和实际阻塞；不要反复讲QwenPaw为什么需要Python，用户已经排除本体范围。

完成时提交给用户：代码和测试变更、实际测试结果、候选FPK及摘要（具备构建条件时）、真机/兼容未验证项、回退条件。没有完成真机门禁时，不宣称全部整改完成或生产可用。

## 十一、参考调查已完成，无需重新大范围搜索

DSH固定提交：`c92f291205f5ebe133b67475145669579353d897`。

已确认：Go常驻控制台，受管上游仍是Node/npm，构建仍有Bash；值得借鉴的是控制权集中、原生argv调用、网关与常驻服务，不是全项目单语言。其版本保留/切换也不等同于用户数据事务回滚。

相关链接均已放入完整计划末尾。
