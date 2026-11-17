# 会话交接：QwenPaw-for-fnOS

- 初始交接日期：2026-11-04；当前状态更新：2026-11-17。
- 本文只保留当前状态、有效边界和下一步。历史平台证据见 `platform-validation-2026-11-03.md` 与 `unified-gateway-migration-plan.md`，不再重复粘贴旧候选摘要。

## 当前候选

- manifest：`1.2.7-7`。
- FPK：`build/candidates/1.2.7-7-orphan-reinstall-20261117/qwenpaw-1.2.7-7-x86.fpk`。
- 稳定路径：`build/candidates/latest/qwenpaw-latest-x86.fpk`。
- 大小：23,773,890 字节。
- FPK SHA256：`382684beadb7c61349d14fe8bea1e5bb63edeb0ce4a2163e8777185d84f57e6c`。
- 包内 musl supervisor SHA256：`d3b0ca12241369a73bb5219d0baa6f2fd7f65e069523bf64c5ce3660f64b91e2`。
- 这是测试候选，不是生产发布。旧候选和重复输出已清理，历史文档中的旧路径仅作审计记录。

## 当前实现

- 生产管理层使用 Rust，控制台使用 Preact + TypeScript；管理页面和上游 Web 均经 fnOS Gateway，生产 CGI 已退役。
- 外壳与内核版本双轨。升级保留可用内核环境，不隐式改变用户配置、记忆或认证。
- 手机端使用固定底栏切换概览、配置、授权与备份、日志；桌面端展示完整单页。
- 工作区目录以 QwenPaw `project_dirs` 为登记来源。授权恢复和连续添加已由用户实机确认可用；解绑不删除文件。
- manifest 按官方 JS SDK 要求声明 `micro_app=true`，打开目录使用 `TrimApp.ready() → openFileManager(path)`。此前 `1.2.6-5` 的双端原生打开验收保持，不自动扩展到本候选手机 App。
- Console 以官方 v2.3.0 `0a21644e90d8ba37e20035b3bfb63880872cc2a2` 为基线，源码补丁 SHA256 `b0ba1b6e48df72f6c3f7188dbee23a33ce282ef605f2f46401c64356f92d8c2f`，动态预加载并发为 16，保留运行时 base-path 与浏览器 Cookie 清理；不再回退下载其他版本源码。
- 历史补丁清单合并各版本 `sha256` 与 `previous_sha256`，覆盖现场旧 venv。`/proc/exe` 权限受限时回退完整 `cmdline` 并规范化两侧路径，PID、starttime 和不匹配拒绝规则不变。
- 全新安装先停止已登记实例；没有登记时，仅接受唯一、路径与 `serve` 参数均匹配的受管 orphan supervisor，二次核验后 SIGTERM。多个实例或身份不明失败关闭，不放宽普通启动规则。
- QwenPaw 2.3.0 使用原生 base-path 补丁与完整预构建 Console，代理保留配置前缀。WebSocket 仍返回 501，不宣称完整支持。

当前管理契约见 `management-contract-inventory.md`，事务与测试设计见 `management-refactor-plan.md`。

## 本轮验证

- 干净源码 Console 为 683 个测试文件、8744 项通过（2 项预期失败）；后端相关 52 项通过。生产构建首屏 2.921 MiB raw、0.792 MiB Brotli。
- Rust 388+13 在启用 loopback 的隔离 user/network namespace 中通过，严格 Clippy 通过。一次测试 socket 冲突经定向及全量复跑后全绿，不计为远程功能回归。
- preflight、完整 FPK 构建和内容审计通过，915 个补丁文件哈希一致；版本同步、`AUDIT_REPORT.md` 和 `git diff --check` 通过。
- 无旧 PID 登记的隔离重装回调 8.9 秒完成，内核 2.3.0 正常运行、认证保留、无需恢复。
- `-7` 保留数据卸载隔离验收通过：supervisor 与内核进程停止、`home/venv` 删除，版本钉、共享数据及认证测试文件保留；随后按 2.3.0 版本钉重装恢复成功。证据 `/tmp/qwenpaw-127-keep-uninstall.json`。
- 远程应用中心安装返回 progress=100/status=2；`1.2.7-7 running`。随后安装内核 2.3.0，PID 745283，`running=true`、`recovery_required=false`。
- 远程主 Web 聊天输入与 `/settings/models` 深链刷新通过，资源/API/页面错误均为 0；资源峰值 18，未写成严格 16。未发送模型请求。

## 待验收与已知边界

- 当前远程验收使用桌面 Chromium 与真实 fnOS 鉴权入口，不等同真实手机 fnOS App 复测；此前 `1.2.6-5` 的双端原生打开结论不自动扩展到本候选。
- 动态预加载本身限制为 16，但深链导航重叠时实测总资源峰值为 18；不宣称全局严格 16。
- fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不称会话已撤销或问题已修复。
- 官方 PyPI 直连曾出现 15 分钟无输出超时；阿里云真实上游安装/卸载通过不等于官方 PyPI 直连通过。
- 历史候选、远端 `running` 和局部平台证据均不能写成当前版本生产发布结论。

## 工作树与操作纪律

- 工作树包含本任务以前的大量未提交源码、文档和删除变更，以及未跟踪的新模块；必须按当前工作树继续，不得只依据 HEAD。
- 不用 reset、checkout 或 clean 覆盖既有修改；不自动 commit、push、tag、发布或部署。
- 隔离测试使用临时目录并核验进程身份；本轮远程安装已经用户明确授权，未发送模型请求或读取用户数据内容。
- `build/`、`web/dist/`、`app/www/`、`app/bin/`、`supervisor/target/` 是生成目录，不手改。
- 当前只保留 `1.2.7-7` 候选、`latest` 副本及对应构建输出；`fpk/qwenpaw-1.2.3-7-proxy-prefix-x86.fpk` 是历史升级回归输入，不属于过期候选，继续保留。
- 2026-11-17 已清理临时源码、测试 venv、过期 FPK 和多余 worktree；`ui-refactor` 已并入 main 后删除。上游未提交改动备份于 `build/cleanup-backup-20261117/`，最新远程与卸载证据归档于 `build/validation/1.2.7-7/`。两个 `/tmp/qwenpaw-payload-*` 目录因属主权限保留，未提权删除。

## 文档索引

- 项目约束与开发命令：根目录 `AGENTS.md`。
- 用户安装、功能与候选：`README.md`。
- 当前管理契约：`management-contract-inventory.md`。
- 整改架构与测试门禁：`management-refactor-plan.md`。
- Gateway 迁移和回退：`unified-gateway-migration-plan.md`。
- 平台证据：`platform-validation-2026-11-03.md`。
- 历史后端提案：`backend-design.md`，不作为当前实现依据。
