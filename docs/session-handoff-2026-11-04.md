# 会话交接：QwenPaw-for-fnOS

- 初始交接日期：2026-11-04；当前状态更新：2026-11-17。
- 本文只保留当前状态、有效边界和下一步。历史平台证据见 `platform-validation-2026-11-03.md` 与 `unified-gateway-migration-plan.md`，不再重复粘贴旧候选摘要。

## 当前候选

- manifest：`1.2.7-10`。
- FPK：`build/candidates/1.2.7-10-console-cleanup-20261117/qwenpaw-1.2.7-10-x86.fpk`。
- 稳定路径：`build/candidates/latest/qwenpaw-latest-x86.fpk`。
- 大小：23,854,964 字节。
- FPK SHA256：`2034c7034047e420ec0669e0463c4948f71d2fdb44e68766330538c7782bc373`。
- 包内 musl supervisor SHA256：`b7229a4c2159627dfc2f74022ef133b0b8452b1987e9a3586e677eeee2dae722`。
- 这是测试候选，不是生产发布。旧候选和重复输出已清理，历史文档中的旧路径仅作审计记录。

## 当前实现

- 生产管理层使用 Rust，控制台使用 Preact + TypeScript；管理页面和上游 Web 均经 fnOS Gateway，生产 CGI 已退役。
- 外壳与内核版本双轨。升级保留可用内核环境，不隐式改变用户配置、记忆或认证。
- 手机端使用固定底栏切换概览、配置、授权与备份、日志；桌面端展示完整单页。
- 工作区目录以 QwenPaw `project_dirs` 为登记来源。授权恢复和连续添加已由用户实机确认可用；解绑不删除文件。
- manifest 按官方 JS SDK 要求声明 `micro_app=true`，打开目录使用 `TrimApp.ready() → openFileManager(path)`。此前 `1.2.6-5` 的双端原生打开验收保持，不自动扩展到本候选手机 App。
- Console 以官方 v2.3.0 `0a21644e90d8ba37e20035b3bfb63880872cc2a2` 为基线，源码补丁 SHA256 `aa2528c63af7078fe4d4d30e37e3eb1d4ae854f99344a9f59e470d0223766441`，动态预加载并发为 16，保留运行时 base-path 与浏览器 Cookie 清理；不再回退下载其他版本源码。`/assets` 通过内核既有 `CompressedStaticFiles` 使用 Brotli/gzip 和一年 immutable 缓存，公共静态文件缓存 300 秒，HTML 保持 no-store。删除清单包含 1,683 个新版不再使用的官方文件（原始 35,112,988 字节），删除前核验官方 SHA256 和实际父目录，变化、越界或符号链接失败关闭。生成器只接受完整 dist，并逐项验证 Vite manifest 引用存在；差异载荷目录会被拒绝。
- 历史补丁清单合并各版本 `sha256` 与 `previous_sha256`，覆盖现场旧 venv。`/proc/exe` 权限受限时回退完整 `cmdline` 并规范化两侧路径，PID、starttime 和不匹配拒绝规则不变。
- 已登记且与当前目标一致的 supervisor 直接复用，路径变化才停止旧实例；没有登记时，需存在 venv 或 Gateway socket 等历史安装证据，且只接受唯一、当前目标路径与 `serve` 参数均匹配的 orphan，二次核验后 SIGTERM。健康内核不因重装被停止，多实例或身份不明失败关闭。
- QwenPaw 2.3.0 使用原生 base-path 补丁与完整预构建 Console，代理保留配置前缀。WebSocket 仍返回 501，不宣称完整支持。

当前管理契约见 `management-contract-inventory.md`，事务与测试设计见 `management-refactor-plan.md`。

## 本轮验证

- 干净源码 Console 为 683 个测试文件、8744 项通过（2 项预期失败）；后端相关 52 项通过，新增覆盖 Brotli、gzip、identity、强缓存、公共文件短缓存与 HTML no-store。生产构建首屏 2.921 MiB raw、0.792 MiB Brotli。
- 真实 2,149,160 字节入口资源经完整 ASGI 应用验证：Brotli 593,644 字节、gzip 668,878 字节，三种编码解码后的内容一致；`/settings/models` 深链保持 no-store。
- Rust 390+13 在启用 loopback 的隔离 user/network namespace 中通过，严格 Clippy 和严格 lint 通过。一次测试 socket 冲突经定向及全量复跑后全绿，不计为远程功能回归。
- preflight、完整 FPK 构建和内容审计通过，915 个补丁文件哈希一致；版本同步、`AUDIT_REPORT.md` 和除补丁文本自身外的 `git diff --check` 通过。
- 无旧 PID 登记的隔离重装回调 8.9 秒完成，内核 2.3.0 正常运行、认证保留、无需恢复。
- `-7` 保留数据卸载隔离验收通过：supervisor 与内核进程停止、`home/venv` 删除，版本钉、共享数据及认证测试文件保留；随后按 2.3.0 版本钉重装恢复成功。证据 `/tmp/qwenpaw-127-keep-uninstall.json`。
- 远程应用中心从 `1.2.7-7` 升级到 `1.2.7-8` 成功；内核 2.3.0 `running=true`、认证保留、`recovery_required=false`。补丁指纹变化触发了内核安装事务，日志出现获取 uv 和安装依赖，不写成无重建升级。
- 同组 80 个资源直连重放：`-7` identity 4,539,601 字节、0.356 秒、CPU 0.340 秒；`-8` Brotli 1,197,132 字节、0.357 秒、CPU 0.380 秒。传输减少 73.63%，单次 CPU 未下降。
- 真实浏览器 `-8` 冷加载 139 个资源、2,929,397 编码字节、页面错误 0；warm 的 139 个资源全部有 immutable，编码传输为 0。`-7` warm 同数为 127/139 有 immutable、625,829 编码字节。冷加载样本的资源数和等待窗口不同，不比较其总耗时。
- 两版各自 10 秒空闲采样均为 0.020 秒 CPU；报告中的启动期高占用不能作为当前稳定空转基线。`-8` warm 另有 1 个无对应响应的 aborted 请求，页面错误为 0。
- 证据汇总：`build/validation/1.2.7-8/summary.json`。未发送模型请求或删除用户数据。
- 中间 `-9` 因把差异载荷作为完整 Console 输入，误删 `internalFileLinks-CvVEwrm1.js` 等必要文件并出现 404，已明确拒绝。`-10` 改用完整生产 dist，并增加 manifest 引用完整性门禁。
- 本地真实 `-8 → -10` 升级回调 21.6 秒，2.3.0 running、无需恢复；Console 为 1,145 个文件，必需资源存在。
- 远程 `-9 → -10` 升级成功，认证 protected、recovery_required=false，聊天页资源/API/页面错误均为 0。资源文件 2,809→1,126，du 242→111 MB，表观 85→52 MB。证据 `build/validation/1.2.7-10/summary.json`。

## 待验收与已知边界

- 本轮真实浏览器为桌面 Chromium；不等同真实手机 fnOS App 复测。此前 `1.2.6-5` 的双端原生打开结论不自动扩展到本候选。
- 动态预加载本身限制为 16，但深链导航重叠时历史实测总资源峰值为 18；不宣称全局严格 16。
- fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不称会话已撤销或问题已修复。
- 官方 PyPI 直连曾出现 15 分钟无输出超时；阿里云真实上游安装/卸载通过不等于官方 PyPI 直连通过。
- 历史候选、远端 `running` 和局部平台证据均不能写成当前版本生产发布结论。

## 工作树与操作纪律

- 保留当前未提交修改；后续继续时先核对工作树，不得只依据 HEAD。
- 不用 reset、checkout 或 clean 覆盖既有修改；不自动 commit、push、tag、发布或部署。
- 隔离测试使用临时目录并核验进程身份；本轮远程升级经过用户明确授权，未发送模型请求或删除用户数据。
- `build/`、`web/dist/`、`app/www/`、`app/bin/`、`supervisor/target/` 是生成目录，不手改。
- 当前候选为 `1.2.7-10`，`latest` 指向 `-10`。保留 `-7`、`-8` 作为历史升级回归输入；`-9` 仅保留作缺陷审计，不可安装。`fpk/qwenpaw-1.2.3-7-proxy-prefix-x86.fpk` 继续作为旧版升级回归输入保留。
- 2026-11-17 已清理临时源码、测试 venv、过期 FPK 和多余 worktree；`ui-refactor` 已并入 main 后删除。上游未提交改动备份于 `build/cleanup-backup-20261117/`，远程证据归档于 `build/validation/1.2.7-7/`、`1.2.7-8/` 与 `1.2.7-10/`。两个 `/tmp/qwenpaw-payload-*` 目录因属主权限保留，未提权删除。

## 文档索引

- 项目约束与开发命令：根目录 `AGENTS.md`。
- 用户安装、功能与候选：`README.md`。
- 当前管理契约：`management-contract-inventory.md`。
- 整改架构与测试门禁：`management-refactor-plan.md`。
- Gateway 迁移和回退：`unified-gateway-migration-plan.md`。
- 平台证据：`platform-validation-2026-11-03.md`。
- 历史后端提案：`backend-design.md`，不作为当前实现依据。
