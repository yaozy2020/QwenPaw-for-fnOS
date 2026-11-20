# 会话交接：QwenPaw-for-fnOS

- `1.4.7-1` 候选：`build/candidates/1.4.7-1-pr8719-picker/qwenpaw-1.4.7-1-x86.fpk`，SHA256 `b5dbfa5b8fb81fe295608770f716c14d7f9ae726193cb96ea736636e7696e510`。跟进 #8719 到 `15614b27` 对官方 post1 适用的本地客户端改动；相比 1.4.6，仅 8 个后端载荷新增或变化，内核 Console 资源未变。包内 1145 个文件摘要全部符合清单。用包内 Chrome 桥接和官方 wheel 在隔离回环服务中验证无前缀、飞牛前缀、空格和中文路径均取得 101、收到 `hello_ack`。Vue 首页使用单一面板内版本列表，选择不发起安装。前端 175 项、Rust 428+13 项、严格 Clippy/lint、包审计通过。包内选择器在 Chromium 和 WebKit 实测，可见列表在 390/1280px 宽度内且每行至少 44px。Chromium 综合 Gateway 12/13：剩余一项在安装、回执丢失与卸载流程的最后一个网络断言失败，除故意丢弃的 POST 外还观测到分区切换中的 SSE GET 取消，不记作全绿。没有执行本版应用中心升级、独立插件或真实 Chrome 扩展验收。

- `1.4.6` 在配置 → 访问鉴权中为飞牛管理员提供原生密码恢复。新密码直接设置给原账号；不进入首次注册，不改桌面绑定。需已启动的匹配适配内核，不做无人值守的真实账号重置；本版尚未经飞牛应用中心升级验收。最终候选 `build/candidates/1.4.6-native-password/qwenpaw-1.4.6-x86.fpk`，SHA256 `00deaf0af3ae3089d20a6309b47df91a7c77ceca9aef7e102c4c7df32c92d208`。包内页面及监督者配合真实原生鉴权路由在隔离 Chromium / WebKit 完成设置新密码、用原用户名进入聊天、旧密码与令牌拒绝、重启后仍生效的闭环，未使用真实用户账号。前端 175/175、Rust 428+13、严格 Clippy 及 FPK 审计通过。额外的三个管理操作组合 E2E 在密码失败提示断言之后，均因未选目标版本而等待禁用的 `#btnUpgradeAction` 超时；不记为整包全绿。

- `1.4.5-1` 恢复安装修复已实机验收：保留数据卸载后将 var 根精确置为 0770，再通过应用中心安装修复包；根目录收敛为 0750，2.3.0.post1 自动恢复且原测试账号登录聊天成功。随后执行删除数据卸载，应用未安装、进程与 18088 停止，应用目录仅剩空目录。候选 `build/candidates/1.4.5-1-retained-reinstall/qwenpaw-1.4.5-1-x86.fpk`，SHA256 `0c930f1ef444509b78d02144ea3760d69d2150743615d4474d88ad21269afac6`。
- `1.4.5-1` 前端调整：概览两处运行时长统一显示精度：满 24 小时从秒折叠为分钟，资源采样不变。前端 174 项通过；从 `build/candidates/1.4.5-1-uptime/qwenpaw-1.4.5-1-x86.fpk` 提取页面与监督者，在隔离 Chromium 中推进浏览器时钟，验证跨天后两处均按整分钟显示。本轮未部署 NAS。
- `1.4.5` 优化管理台首次打开、手动刷新、概览资源读秒与启停反馈；不改变四分区、原生鉴权、内核版本钉或停止核验。版本元数据慢时先显示进程，版本标为「待确认」。本版未在应用中心升级，继承未决发布门禁。
- `1.4.5` 候选：`build/candidates/1.4.5-monitor/qwenpaw-1.4.5-x86.fpk`，33,912,590 字节，SHA256 `2989bde9912b87378a5ed8b8d7eddd09f9669c30ddadfc89a83ebb0d53564493`。最终包内页面与 supervisor 接入隔离 Gateway 和真实受管测试进程。Chromium / WebKit 各 3 项通过：同一慢探针条件下，从导航到已安装状态分别 111 / 207 ms；手动刷新 60 / 48 ms；连续两次资源帧间隔分别 999、1000 / 1000、999 ms，期间没有额外全量状态 GET；启/停响应到确认显示为 5/3、6/6 ms。这些是单次本地观测，不是 fnOS 桌面、真实 Python 冷启动或远程网络的延迟保证。
- 上一候选 `1.4.4`，在 `1.4.3-2` 子路径修复上优化 Vue 首屏挂载。首次打开配置或授权与备份才创建对应控件，切换后保留；不改布局和后端。本版未做飞牛应用中心升级，继承未决发布门禁。
- `1.4.4` 本地候选 `build/candidates/1.4.4-vue-panel/qwenpaw-1.4.4-x86.fpk`，SHA256 `ba8d88c3bdd469fbf2bf4b4f54a318d57be827f5136f121eabf38ee8996205d5`。同一健康状态夹具中，优化前与包内首页 DOM 元素数为 329 / 274；这不是首屏耗时或内存测量。前端 172/172、类型检查、包审计通过。使用最终包内页面和监督者测试版本选择、备份说明往返及日志追加，隔离 Chromium / WebKit 各通过 2 项。
- `1.4.3-2` 远端实机：全新安装、桌面注册/登录/刷新、1138 个补丁摘要、doctor 与 Chrome URL、保留数据卸载通过；恢复安装因 var 被平台改为 0770 而失败。删除数据卸载未执行，Data 与 Chrome 扩展安装器不在此 FPK 内。不可作为后续版本或完整插件验收证据。
- 上一轮 `1.4.3-1` 的本机真实 Chrome / fnOS 桌面验收：原生会话进入带前缀的聊天页，刷新保持路径；Creator、Data 未安装，语音/WS 未执行。不可作为 `1.4.4` 的实机证据。
- `1.4.3` 退役新设反代密码及空密码清除。保留旧密码直到原账号核验并明确切换。不改变原生首次注册、本机服务端口与飞牛入口绑定；继承以下未决发布门禁。
- 1.4.3 本地候选 `build/candidates/1.4.3-native-auth/qwenpaw-1.4.3-x86.fpk`，SHA256 `92b9e9a3874f5a69d792cb1ba98a5cbc45cb97751df957ae30a92644590084c3`。最终包通过内容审计和隔离 Chromium 原账号登录/退役旧密码回归；前端 166 项、Rust 限并发 424+13 项。此前 WebKit 流程失败：隔离夹具以 shebang 脚本假扮 venv 解释器。PID 缓存过期后强制读取状态，可稳定得到内核 HTTP 200 而监督者报告 PID 0。夹具改为 venv 路径下的真实解释器启动 `-m qwenpaw`，保留到期后重新识别的断言。未调整生产身份核验、未换包。最终 FPK 在 Chromium 与 WebKit 上均通过原账号登录、旧密码升级、取消/失败保留、确认/重启与敏感业务请求拒绝的隔离回归；不是 fnOS 真机验收。隔离证据 `build/validation/1.4.3-native-auth/confirmed-{chromium,webkit}.log`。
- 1.4.3 飞牛测试机升级：经官方 HTTPS 应用中心手动上传同一 FPK；升级前 manifest 为 1.3.5-3，原生账号已初始化；升级后 manifest 为 1.4.3、内核返回 2.3.0.post1，已安装 supervisor SHA256 `d07d2fa0e19b288cac33c841b2cdd22d3fbb9925689e7de9472990b95d91fb59` 与包内一致。管理员从应用中心打开控制台，在「配置」读取到原生账号、管理员已创建、外部监听未选中。两个实际入口均仅监听回环，分别使用原生会话验证：匿名 401、原账号登录及核验均为 200。未修改测试机外部访问范围、未专门构造旧密码、未测试数据恢复或长稳；测试机结果不替代这些门禁。未正式发布。
- `1.4.2` 新增可选安装前 tar 备份及离线恢复指引；概览常显统一版本选择框。继承以下未决发布门禁。
- 1.4.2 候选 `build/candidates/1.4.2-panel-backup/qwenpaw-1.4.2-x86.fpk`，SHA256 `ffe2c44f5e5a1c249b8a8d512643442557a75de1aa11a63e63676efefd012986`。前端 165/165，类型检查、Clippy 与 FPK 审计通过；从包内取 supervisor 和首页，在隔离 Gateway 上 Chromium / WebKit 各通过新流程 1 项。Rust 限并发 424+13 全过；默认并发首轮有两项非本次功能的失败（启动测试脚本 ETXTBSY、日志订阅空闲断言），不视为已修复。备份测试经过失败的安装器、离线解 tar 和重复安装不覆盖验证。证据在 `build/validation/1.4.2-panel-backup/`，不是手机真机或完整发布验收。
- 上一候选：`1.4.1`，未发布；Vue 与 DSH 的功能边界、保留策略及差异见[管理契约](management-contract-inventory.md#141-与-dsh-的功能对照)。下文 `1.3.x` 的候选、摘要及平台验证均为历史记录，不属于 `1.4.1` 的真机证据。
- 1.4.1 发布暂缓：面板 163 项、Rust 422+13 项以及包内密码迁移通过，但隔离 Gateway 联调出现安装子进程身份未能确认、事务进入恢复状态。不要绕过身份检查或把重跑成功当作修复。证据在 `build/validation/1.4.1-vue/`；未做本版飞牛真机升级。
- 本文只保留当前状态、有效边界和下一步。历史平台证据见 `platform-validation-2026-11-03.md` 与 `unified-gateway-migration-plan.md`，不再重复粘贴旧候选摘要。

## 当前本地专项

- 当前修复候选为 `1.3.3-2`，以官方 `2.3.0.post1` 为适配基线，只携带一套完整 Console；健康 2.3.0 在升级时经现有 venv 事务迁移到 post1，失败回滚并保留用户数据。镜像明确缺少 post1 时，仅本次迁移使用官方 PyPI，原镜像设置不变。
- 登录页不再初始化聊天插件；管理日志按 50 行分组跳过屏外绘制。post1 登录页实测 24 个资源请求，真实 Rust 反代与 WebKit 隔离测量通过。
- FPK `build/candidates/1.3.3-2-mirror-fallback-20261118/qwenpaw-1.3.3-2-x86.fpk`，33,512,246 字节，SHA256 `a72ab37a62f23a0440ecefa78371d48696d73f212c20437c594cb6ca2446e575`。post1 Console 693 个测试文件、8926 项通过（2 项预期失败），Rust 406+13、静态路由 31、平台 51 项及 FPK 审计通过。
- 本机已安装 `1.3.3-1` 并迁移到 post1，`1.3.3-2` 尚未部署；真实 iOS、生产网络与 24 小时稳定性未覆盖。旧候选结果只作为历史证据。
- 本轮不自动 commit、push、发布或部署；只读现场和独立目录、端口、容器的隔离测试不等同于授权替换已安装服务。

## 历史候选（1.2.7-7）

- manifest：`1.2.7-10`。
- FPK：`build/candidates/1.2.7-10-console-cleanup-20261117/qwenpaw-1.2.7-10-x86.fpk`。
- 稳定路径：`build/candidates/latest/qwenpaw-latest-x86.fpk`。
- 大小：23,854,964 字节。
- FPK SHA256：`2034c7034047e420ec0669e0463c4948f71d2fdb44e68766330538c7782bc373`。
- 包内 musl supervisor SHA256：`b7229a4c2159627dfc2f74022ef133b0b8452b1987e9a3586e677eeee2dae722`。
- 这是测试候选，不是生产发布。旧候选和重复输出已清理，历史文档中的旧路径仅作审计记录。

## 当前实现

- 生产管理层使用 Rust，控制台在 1.3 候选使用 Preact + TypeScript，1.4 工作树已开始迁移至 Vue 3 + TypeScript；管理页面和上游 Web 均经 fnOS Gateway，生产 CGI 已退役。
- 外壳与内核版本双轨。升级保留可用内核环境，不隐式改变用户配置、记忆或认证。
- 手机固定底栏和桌面侧栏切换概览、配置、授权与备份、日志；版本管理常显在概览内，`#version` 是该卡片的深链。
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
