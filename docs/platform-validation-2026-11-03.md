# fnOS 平台验证与阻塞记录（P0）

- 初始记录日期：2026-11-03；支持版本决定：2026-11-04；本次状态更新：2026-11-09。下文详细 G0–G3 证据截至 2026-11-08，保留为历史记录，不覆盖本节当前状态。
- 当前状态：G0–G3 按已批准范围完成；真实手机 fnOS App 暂不纳入本轮验收，最终手机复测仍待用户。G4 已实施，当前 Gateway-only：生产 CGI 文件、子命令、前端 action fallback、CGI 专属测试及 CI 门禁已删除。管理控制台与 `qwenpaw.main` 均经 `/app/qwenpaw/web/` 进入同一 Gateway/Socket，不直连内核端口，不使用 `target blank`。Rust `web_proxy` 适配上游路径、流式 SSE、Bearer Authorization，并区分管理 CSRF 与上游请求；WebSocket 返回 501，不宣称完整支持。
- 当前测试候选 FPK：`build/candidates/1.1.1-11-dual-web-proxy-20261109/qwenpaw-1.1.1-11-x86.fpk`，SHA256 `2db563daa353fca8ac1ed6c59275375787eb1fae37abc722a49434a8098723c1`；supervisor SHA256 `b7813f1ab26315599035acfd042656c82634624c225005c1f0a36d1758bb3f6f`。远端为 `1.1.1-11 running`，内核 `2.3.0b2 running`，无活动事务；不是生产发布。独立TCP反代自动分配、冲突回退、前端可改且先bind再切换，当前33083；独立图标无port并走Gateway。双入口真实Chromium鉴权/中文/en保留/无启动超时通过，gzip生效。Rust 283+13、Clippy、Node 41、typecheck、build/audit通过，真实手机复测仍待用户。
- G3 会话边界：fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不称其已撤销或已修复。官方 PyPI 直连 15 分钟无输出超时；真实阿里云 `qwenpaw==2.2.2.post1` 上游安装/卸载通过，但不能将其写成官方 PyPI 直连通过。
- 前端合法 `seq + instance` 使用 `sessionStorage`；页面重建/pageshow 恢复同一任务轮询、不重复 POST，存储不可用时安全降级。语言仅在首次初始化时默认中文，不覆盖已有英文。
- 历史 1.1.1-3 远端平台状态及对应证据目录：`/tmp/qwenpaw-g3-1.1.1-3-20261108/`；其中 A/B/C/D 及范围内验收结论保留为历史事实，不应把旧候选摘要当作当前 FPK。历史只读/探针和 logout 记录按其采集时间解释；凭据不记录在本文。

## G1 本地实现与源码/黑盒回归证据（不等同 fnOS 验收）

- 独立 Gateway UDS 与私有 `supervisor-control.sock` 分离；本地服务以 accept 时采样的 root UID 作为生产 Gateway 边界，并要求管理员角色，变更 API 使用 HMAC 签名的 double-submit CSRF。源码与隔离黑盒回归覆盖伪造/缺失身份、非管理员和无效 CSRF 拒绝；本地 root UID 夹具不代表真实 fnOS peer 实测。
- 网关和 CGI 静态资源共用 `web_static`；Socket bind 后固定本实例 inode 身份，路径被替换时拒绝并避免误删其他 Socket。SSE 配额为 8，总连接上限 32，超限返回 HTTP 503 与 `Retry-After: 1`；service readiness 检查真实 UDS、HTTP 响应和 supervisor 身份。
- G1 历史候选 SHA256 `e3aa519a5907f2dc7fa96a04d7aef9cf4ee5271832e42fda2c2c72695ecad1f9`，当时包内 supervisor SHA256 `28df3174eff0f4b8b3882bb331bd680843441a5d4803690b787b92665f5b0e75`，Gateway 16/16、管理 API 和七点恢复均通过。

## G2 本地候选与初始平台状态（历史阶段记录；不等同 fnOS 验收）

- 候选桌面入口/前端已切换 Gateway，CGI 仅保留回退物料。主控实际通过 Rust 264+13、Node 35、严格 Clippy/typecheck/build、Gateway UDS 17、Chromium Gateway 2 与 CGI 1、完整 STRICT_CI lint、build/audit/preflight；包内 supervisor 重跑 Gateway 17、管理 API 与七点 SIGKILL recovery 均通过。
- G2 历史候选 `build/output/qwenpaw-1.1.1-1-x86.fpk` 大小 2,141,266 字节，SHA256 `c56922738f654dec48ad48b1f675f9de8fc00f4e0bc47f7b8983006c35a4d336`；包内 supervisor SHA256 `3b53b4c6307e87cbf6fc5ae16da3f62594c8739ada883165d88cd41add387df2`，test-runtime SHA256 `6d8a192872e65442e78875cae86b2f2fafcd06b21cfd054dae8c4ed510a2b88c`。以上为本地 G2 历史候选，不是当前远端最终包。
- 本地隔离 VM 的 QEMU 身份、HTTPS/SSH 转发与公开 fnOS 登录页只证明当时可达；该次核实时浏览器桥未连接且没有可复用登录态。此历史边界不覆盖当前获授权远端实例的正式 UI 验收，也不把本地回归写成生产证据。

## 历史记录：2026-11-07—08 G3 验收快照（A/B/C/D 通过；当时整体未退出）

- 截至 2026-11-08 的历史候选为 `build/candidates/1.1.1-3-gateway-g3-20261108/qwenpaw-1.1.1-3-x86.fpk`，FPK SHA256 `94679e4d8343c1bf341e8a1cc2a1451a9ad0ad3296d2fc668deab456db601b51`；包内 supervisor SHA256 `2829f1a4079139f166f78478de804d3f619226d646ea93d58d0965718dbc4400`。该包已从 1.1.1-2 经官方 update 单次升级并在远端验收；1.1.1-2/c571 为历史本地候选，667 为更早远端包。当前 1.1.1-6 测试候选及 SHA256 见本文顶部；`running` 不代表生产发布。
- [x] A 完整 GET/HEAD/query/404/编码拒绝/Content-Type/SSE 断连矩阵通过；B 官方连续 `stop/start/start/reconnect` 通过；C 真实 app.sock stale/active listener/普通文件/软链接/unknown owner 均 fail-closed 并恢复。
- [x] D 阿里云真实 `qwenpaw==2.2.2.post1` 安装、健康、stop/start/stop、仅卸载通过；新包 manager 一次 SIGKILL → 一次官方 start → 原内核 PID/starttime/exe 接管、sentinel 存活，随后停止并卸载；remove 范围和同一 FPK 官方重装恢复通过。
- [x] 最终 App Center `1.1.1-3 running`，唯一 manager、Gateway HTTP 200、无 QwenPaw 内核/未决事务/`18088` listener，双 UDS mode `0600`、UID/GID `994/989`。匿名/伪造页面/API/SSE 均为平台 HTTP 200 `text/plain` `invalid token`，不构成公开 HTML 或授权通过。证据总目录：`/tmp/qwenpaw-g3-1.1.1-3-20261108/`。
- [ ] 真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞。新 1.1.1-3 Playwright Chromium UI logout 证据（`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`）显示“我的账号→退出登录”导航至 `/login` 并重新登录成功，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；这是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试，真实失效会话门禁仍未通过。官方 PyPI 直连、完整生产路径/错误矩阵和发布门禁仍未完成；截至 2026-11-08 的历史门禁结论为当时 G3 尚未整体关闭、G4 尚未开始。
- 最终回调顺序固定为：先检查可执行的 `${TRIM_PKGVAR}/qwenpaw-supervisor-helper`，再回退到回调同级的 `qwenpaw-supervisor` sibling；两者都不存在或不可执行时返回 `127`。upgrade_callback SHA256 `b6cb930af324cf7d255ccfcc1c6c75e55c432017c5babb2f180507f542be130a`，uninstall_callback SHA256 `c6cdc62e7056dd176c35f8f9317af7069f03a14e0b566354fccd63d57dba947f`；candidate-audit 与 test-platform-callbacks 均 `ALL CHECKS PASSED`。真实 fnOS `@apphome`、`@appconf`、`@appdata` 路径校验已修复并纳入旧候选；这不改变 callback 由已安装包提供的升级时序限制。
- 历史 `905e` 与 `59023c` 原位升级限制仅作历史说明：升级时 callback 由已安装的旧包提供，不能假设新包 callback 已生效；`59023c` 的 sibling-first 选择及真实路径校验因此导致原位升级失败。为重建测试基线，曾仅将 root 所有的旧 sibling 原子改名，以强制回调走 var helper，再正式执行 keep 卸载/安装；这是测试 fixture 准备，不是历史原位升级通过证据。
- 历史阶段（1.1.1-2）正式 UI 流程已通过 helper-first 1.1.1-1 基线安装并保持 `running`，再从 1.1.1-1 升级到 1.1.1-2；升级 update/task、update/status 均 HTTP 200、code 0，状态为 `upgrading → starting → running`。向导默认阿里云镜像源，该阶段只更新飞牛外壳，不改变已安装 QwenPaw 内核版本。截至 2026-11-08 的 1.1.1-3 已由官方 update 单次升级完成。
- 历史 1.1.1-2 keep 卸载已通过正式 UI；共享根 pre/post exact metadata 均 1,119 行、171,041 字节、SHA256 `e10819ba4e6bbc230a2474ff11dea0b8a0797e8a9ee209e02764242f0643ff75`，added/removed/changed 均为 0（比较 path/type/mode/owner/size/mtime）。凭据 metadata 记录数前后均为 1、`contentRead=false`，`supervisor-token` 卸载后仍存在；但 appdata runtime metadata 由 18 行变为 16 行，不能声称整个 appdata metadata 完全不变。截至 2026-11-08 的 1.1.1-3 的 remove/重装范围见当前证据目录。
- 历史 667 远端只读复核（1.1.1-2）显示 App Center installed HTTP 200、code 0、版本 1.1.1-2、状态 `running`；download HTTP 200、code 10030；Gateway status/logs 均 HTTP 200，badge 为“内核未安装”、version 为“未安装”，且不含“状态不可达”，三个服务按钮均 hidden/disabled。同一未刷新 Gateway observer 的 stop/start 门禁已通过：stop 后应用为 `stopped`、远端 manager 与 `app.sock` 消失，start 后恢复 `running`，manager PID 395874、`/vol1/@appcenter/qwenpaw/app.sock` inode 103；独立 start/check 恢复证据也为 HTTP 200、code 0。截至 2026-11-08 的 1.1.1-3 远端状态见顶部及当前证据目录。
- 历史 1.1.1-2 远端身份与 Socket metadata（manager 进程数 1、PID 395874，/proc exe 为 `/vol1/@appcenter/qwenpaw/bin/qwenpaw-supervisor`）：`/vol1/@appcenter/qwenpaw/bin/qwenpaw-supervisor` mode 775、`/var/apps/qwenpaw/cmd/qwenpaw-supervisor` mode 755、`/var/apps/qwenpaw/var/qwenpaw-supervisor-helper` mode 750，三者大小均 4,435,352 字节且 SHA256 均为 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`；正确 gateway socket `/vol1/@appcenter/qwenpaw/app.sock` 为 mode 600、inode 103，正确 control socket `/vol1/@appdata/qwenpaw/supervisor-control.sock` 为 mode 600、inode 89。`/var/apps/qwenpaw/supervisor-control.sock` 缺失只是旧/错误路径，不能据此声称正确 control socket 缺失；kernel markers 均 absent。final logs metadata：`supervisor.log` 30,167 字节、`qwenpaw.log` 0、`qwenpaw-config.log` 2,733、`qwenpaw-callback-trace.log` 7,102、`qwenpaw-upgrade.log.pip` 0。
- 历史 1.1.1-2 `/vol1/@appdata/qwenpaw/pip-mirror` metadata：pre/final 均 mode 750、uid/gid `994/989`、大小 40 字节、SHA256 `5e2868300962c78938530abb33c3c3d7a724086a6f4a661ad369ce87487945f`，final inode 84；未读取正文。shared-root exact metadata 只证明 metadata 一致，未读取或哈希共享根文件内容，不能扩写为内容一致。
- 历史受支持回退链（1.1.1-2/667 阶段）精确低版本目标为 helper-first 1.1.1-1；先以本地审计确认 1.1.1-1 与 1.1.1-2 包内 supervisor 完全相同，SHA256 均为 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`，内层成员集合相同，差异仅为 manifest 版本/校验、HTML 版本显示及其压缩物；未发现 schema/layout 变化。前置无未决事务。随后通过官方 UI 保留数据卸载当前 1.1.1-2：卸载返回 HTTP 200/code 0，保留 `@appdata`、`@appconf`、`@apphome`、共享配置和 `supervisor-token` metadata，并移除 package target；这里不宣称整个 appdata metadata、runtime 文件或 socket 在生命周期中不变，也未读取或校验共享文件内容。卸载后安装精确旧包，上传 SHA256 为 `2ecbe8bdd8ab7b7710285791a403b33373a9d612040478ec4f1a47452abcf5a8`，App Center 1.1.1-1 为 `running`；Gateway 页面、`status`、`install/status` 与 `logs` 均 HTTP 200，`recovery_required=false`，manager 单一，正确的 `app.sock`/control socket 正常，无内核与未决事务。旧版验收的首次进程查询为 0，随后 `PROCESS_EXACT_RETRY` 得到 `PID_COUNT=1` 且 supervisor SHA256 为预期值，按 retry 结果计。之后仅提交一次官方 update/task，update/status 终态为 5，最终 1.1.1-2 `running`；最终 FPK SHA256 为 `667193f2053739a6263f0ddc125e5edf6a32ac05f11594c9be6c34ba5b06bf92`，四处 supervisor/helper SHA256 均为预期值，最终 Gateway 与数据/事务状态通过。完整受支持回退链已通过；原始记录位于 `/tmp/qwenpaw-g3-supported-rollback-20261107/`。
- 此前从 1.1.1-2 直接原位安装精确低版本的提交动作虽 download/task HTTP 200、code 0，但 fnOS 原文为“无法安装 QwenPaw”“已安装相同或更高版本的 QwenPaw”，最终仍为 1.1.1-2 `running`、download code 10030。该响应证明直接原位降级按设计被禁止，不是平台异常、不是本次受支持回退链的失败或替代证据；同版本最终包重试同样被拒绝。不得再次尝试直接原位降级。
- **历史 1.1.1-2 正式 SIGKILL → 官方恢复子项：通过。** 17:38:02.180 的 `fresh_pre_sigkill_readonly_gate` PASS，唯一 manager PID `459807`、starttime `23000247`，binary SHA256 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`；预期双 UDS、pidfiles 等门禁通过。17:42:55.090 仅一次 `sigkill_once` exit 0，紧接 17:42:55.091 一次官方 `crash_recovery_start_request` HTTP 200/code 0；恢复身份与 `crash_recovery_success` 均 PASS，新 manager PID `462058`，App Center `running`、Gateway HTTP 200、双 UDS 监听及身份/pidfiles 正常。该结果只关闭一次 SIGKILL 后一次官方 start/start 恢复子项；早期探针/身份不匹配尝试不计入此正式结果。
- **历史 1.1.1-2 普通系统重启自动接管子项：通过。** 通过普通 `sudo reboot` 前置门禁；boot_id 从 `2f296c6f-0e40-4a6d-956d-2d08fa27b96e` 变为 `24485936-0f2d-40a9-8017-cd658dab4dc1`。平台日志 18:05:51 记录 `APP_AUTO_STARTED` 且成功注册 `qwenpaw.console`，未手工 start。18:26:57 使用全新认证上下文进行 GET-only 只读复核：App Center HTTP 200/code 0，`1.1.1-2 running`；Gateway 页面、status、install-status、install-log、logs 均 HTTP 200；唯一 manager PID `2697`、PPID 1、binary SHA256 与上项一致；两个预期 UDS mode `0600`、UID/GID `994/989` 且由该 manager 监听；kernel processes 为空、install phase `idle`、`recovery_required=false`。18:29:55 的 `automatic_reboot_gate` PASS，涵盖 boot_id、App Center、Gateway、manager/Socket 身份和无孤儿内核/未决恢复事务。该证据不代表掉电持久性、真实手机 QwenPaw App 验收（本地 WebKit 已通过）或完整平台矩阵。
- **独立 TCP listener 核查：限定范围通过。** `ss -ltnpe` 的服务/内核级检查未发现 QwenPaw TCP listener，两个预期 UDS 正常；当前 SSH 用户无法用 `/proc/*/fd` inode-to-PID 映射，因此这不是完整全系统逐 PID FD 审计，也不扩大为全系统监听归属结论。详见 `/tmp/qwenpaw-g3-lifecycle-20261107/lifecycle-blocked.txt`。
- **截至 2026-11-08 的历史 G3/A/B/C/D 验收：**C 的独立陈旧 `app.sock`、active listener、普通文件、软链接和 unknown owner 均 fail-closed 并恢复，B 的连续 stop/start/start/reconnect、D 的 manager 接管、remove/重装和真实上游均通过。该日真实手机 fnOS App 未纳入 G3；UI logout 后旧 `ost` 会话仍可访问有效 Gateway page/status/SSE；官方 PyPI 直连、完整生产路径/错误矩阵和发布门禁仍未完成，当时记录 G3 尚未整体标完成、G4 尚未开始。**截至 2026-11-09：**G3 按已批准范围完成，G4 已实施并退役生产 CGI；手机复测仍待用户且不阻塞当前验收，logout 平台现象未修复。

### 2026-11-08 本地 Gateway / WebKit / process identity 复验（隔离环境）

- 本地故障矩阵针对 release SHA256 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`：`unshare --user --map-root-user cargo test ... gateway::` 23/23；`python3 scripts/test-gateway-uds.py` 为 23 项 OK、2 skipped（该次 namespace 无法 chown foreign owner；root-mapped 下普通 UID 权限用例跳过）；另 `python3 scripts/test-gateway-uds.py --permission-only` 以普通 UID 1/1 通过。随后主控实际运行隔离 Docker `--user 0 --cap-add CHOWN` 目标回归，foreign-owner stale、reconnect、service readiness 3/3 通过、0 跳过；`test_service_start_stop_restart_reconnects_gateway` 的连续 start、SSE EOF/reset、0600 control socket 不可连接、管理 TCP 关闭、restart 后 Gateway HTTP 200 和 sentinel 存活断言均通过。证据目录 `/tmp/qwenpaw-g3-local-socket-20261107/`。这些是本地覆盖，不代表 fnOS 真机专属 Socket 或完整矩阵通过。
- `supervisor/src/install/process.rs` 已修复短命子进程登记竞态：仅同一身份已退出时允许 executable 缺失，存活或身份变化仍失败关闭。RED/GREEN 证据 `/tmp/qwenpaw-g3-process-identity-20261107/`；install 87、runtime 5、严格 Clippy、user namespace 全 crate 276+13 通过。
- WebKit engine 选择已加入 `web/playwright.config.ts`、`web/e2e/run.mjs`、`web/package.json`、`web/e2e/README.md`；隔离 Docker WebKit Gateway 2/2、Chromium Gateway 2/2 均通过，日志 `/tmp/qwenpaw-gateway-webkit-20261107/e2e-webkit-https.log`，Chromium 最终输出可写主控复验通过。WebKit 丢回执不是生产 beacon 失败：第三次 beacon 返回 `success=false`，复用 seq/instance、不新建事务；测试严格检查 done/upgrading=false/recovery=false、marker/pin/健康，不依赖可选 pip 日志，跨引擎 console 错误分别精确匹配。

### 2026-11-07 历史 667 远端 GET-only 网关只读矩阵

证据为 `/tmp/qwenpaw-g3-gateway-readonly-20261107/gateway-readonly-matrix-20261107.jsonl`（42 行）及其 summary；全轮没有写请求。目标仍为 `1.1.1-2 running`，无内核，install phase idle、无 recovery。登录 HTTPS 下完整前缀、尾斜线/query、页面、`status`/`install-status`/`install-log`/`logs` 与 HEAD 均为 HTTP 200；SSE snapshot 首帧及断连重连通过。编码穿越、encoded slash、双斜线、dot segment 返回 400 或 404。SPA deep path、未知静态资源/API 均 404；现有 `web_static` 是明确的静态白名单且无 SPA fallback，本条记录为观察及当前契约，不宣称 deep-link 200，也不据此判为生产缺陷。匿名根与匿名 SSE 控制请求返回 `200 text/plain`、13 字节且无 snapshot；这不等于公开 HTML，也不构成登录/管理员授权矩阵通过。首次匿名 SSE 的 `status=0` 是工具参数错误，纠正后不计为平台失败。响应仅观察到 `nosniff`；未观察到 HSTS/CSP/XFO/Referrer/Permissions-Policy，TLS 证书信任未验证。

## 平台门禁与需要的证据（历史矩阵截至 2026-11-08；当前状态以本文顶部为准）

| 门禁项 | 当前状态 | 仍需保留或补齐的边界（仅在可丢弃的 fnOS 测试实例和隔离数据上取得） |
|---|---|---|
| 最低 fnOS 版本 | 用户已批准 `os_min_version=1.2.0800`，manifest 已更新；最低与本次目标版本相同。低于 `1.2.0800` 的旧系统不再承诺 CGI 兼容支持。 | 不把支持下限调整解释为回退通过；继续保留原位降级禁令证据。 |
| 统一网关、路径前缀 | 1.1.1-3 A 已通过完整 GET/HEAD/query/404/编码拒绝/Content-Type/SSE 断连矩阵；G0 探针证明完整 `/app/qwenpaw` 前缀、尾斜线及查询串原样到达，Gateway 页面/API 最终 HTTP 200。 | 真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；新 UI logout 的旧会话撤销仍未通过，完整生产客户端路径仍受门禁约束；本地 WebKit/Chromium 保留为浏览器兼容证据。 |
| 网关 UDS / Socket 生命周期 | 1.1.1-3 B 的连续 `stop/start/start/reconnect`、C 的 stale/active listener/普通文件/软链接/unknown owner fail-closed 与恢复、D 的 manager 接管及 remove/重装均通过；最终双 UDS mode `0600`、UID/GID `994/989`，Gateway HTTP 200。 | 正常重启已有子项证据，但掉电持久性、超出 A/B/C/D 的完整 manager/孤儿内核错误矩阵仍需补齐；手机 App 暂不纳入本轮 G3，本地隔离矩阵不替代完整生产门禁。证据总目录：`/tmp/qwenpaw-g3-1.1.1-3-20261108/`。 |
| SIGKILL 后官方恢复 | 1.1.1-3 D 已验证新包 manager 一次 SIGKILL → 一次官方 start → 原内核 PID/starttime/exe 接管，sentinel 存活，随后停止并卸载；历史 1.1.1-2 的精确 PID/starttime `459807/23000247` → `462058` 生命周期子项也通过。 | 一次事件不替代连续故障、启动失败及超出 A/B/C/D 的完整 manager/孤儿内核矩阵；当前证据：`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-orphan-followup.json`。 |
| 普通系统重启自动接管 | 历史 1.1.1-2 的普通 `sudo reboot` 后 boot_id 已变化；平台自动记录 `APP_AUTO_STARTED` 并成功注册 `qwenpaw.console`，无手工 start，唯一 manager PID `2697`/PPID 1、正确双 UDS、kernel processes 空、phase `idle`、`recovery_required=false`；`automatic_reboot_gate` PASS。 | 仅证明一次正常系统重启后的自动接管，不代表掉电持久性或完整生产故障矩阵；手机 App 暂不纳入本轮 G3。原始记录：`/tmp/qwenpaw-g3-lifecycle-20261107/system-reboot-acceptance-20261107.jsonl`。 |
| TCP listener 独立核查 | `ss -ltnpe` 服务/内核级检查未发现 QwenPaw TCP listener，两个预期 UDS 正常。 | `/proc/*/fd` inode-to-PID 映射对该 SSH 用户不可用；这是受限范围核查，不是完整全系统逐 PID FD 审计或全系统监听归属证明。记录：`/tmp/qwenpaw-g3-lifecycle-20261107/lifecycle-blocked.txt`。 |
| 网关鉴权、管理员授权与 CSRF | 前序管理员/普通用户/CSRF 证据保留；1.1.1-3 auth gate 对匿名/伪造页面、API、SSE 均观察到平台 HTTP 200 `text/plain` `invalid token`，没有公开 HTML 或业务授权通过。 | 新 1.1.1-3 UI logout 已导航至 `/login` 并重新登录，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；这是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试，真实失效会话门禁仍未通过。匿名/伪造拒绝仍不得扩写为授权通过，不记录 token/cookie。 |
| `install_init` 与安装阶段执行窗口 | 1.1.1-3 已经官方 update 单次升级并完成远端验收；helper-first 回调顺序、真实上游安装/健康/stop/start/stop、仅卸载及最终 remove/重装证据通过，缺 helper/sibling 时仍 fail-closed `127`。 | 继续保留各回调 cwd、用户、TRIM 环境及退出码证据；完整回调重试/停止预算仍是发布门禁，不用历史探针替代正式结论。 |
| 外壳 upgrade 回调窗口 | 1.1.1-2 → 1.1.1-3 正式 update/UI 升级已通过，状态序列与最终 Gateway 入口保持有效；remove 范围、同一 FPK 重装恢复和截至 2026-11-08 的 `1.1.1-3 running` 已核对。 | 直接原位安装低版本按设计被平台拒绝，不能作为有效回退演练；真实失效会话、官方 PyPI 直连和完整生产平台门禁仍未关闭。 |
| 受支持的精确低版本回退 | 历史 1.1.1-2/667 阶段已通过完整链路：确认无未决事务及 schema/layout 兼容，官方 UI 保留数据卸载当前版，安装并验证精确旧包，再仅提交一次官方升级回到当时的 1.1.1-2；原始记录见 `/tmp/qwenpaw-g3-supported-rollback-20261107/`。 | 该回退项已通过；不得再次尝试直接原位降级。截至 2026-11-08 的 1.1.1-3 的 A/B/C/D、remove/重装和生产门禁分别按当前证据记录，手机 App 不在本轮 G3 范围内。 |
| uninstall callback 与 keep/remove 数据范围 | 1.1.1-3 remove 一次删除 target/appdata/appconf/apphome/shared/auth 测试数据及 helper/token/socket，范围外 sentinel 与其他应用不变；随后同一 FPK 官方重装恢复，stop/卸载通过。 | 历史 keep 证据仍只证明既定保留范围的路径/metadata；完整删除不可逆，runtime 文件和 Socket 会随生命周期变化，不扩写为整个 appdata 元数据或共享内容不变。 |
| 真实应用授权与桌面注册（历史快照，截至 2026-11-08） | 当时 1.1.1-3 最终 App Center 为 `running`，唯一 manager、Gateway HTTP 200、无内核/未决事务；前序桌面 Gateway 与当轮 A/B/C/D、remove/重装均通过。 | 当时手机 App 不纳入 G3；UI logout 后旧会话仍有效，官方 PyPI 直连及完整生产发布门禁未完成。该行的 G4/CGI 状态仅描述 2026-11-08：截至 2026-11-09，G4 已实施、生产 CGI 已退役；当前 G3 按批准范围完成。手机复测仍待用户且不阻塞当前验收；本地 WebKit/Chromium 仍仅为浏览器兼容证据。 |

## 包摘要证据（仅历史包）

本轮重新计算：

```text
$ sha256sum build/output/qwenpaw-2.1.0-9-x86.fpk
ba16f40b987fbe8a795fe584747396d0af0b06801fa8749846a8dea331bde623  build/output/qwenpaw-2.1.0-9-x86.fpk
```

该摘要与交接记录中的历史候选一致。`qwenpaw-2.1.0-9-x86.fpk` 是此前事务化改造的历史 FPK，**不是当前测试候选**，不能用来声称当前候选已打包、已测试或适用于当前平台；当前测试候选 1.1.1-6 的路径和 SHA256 见本文顶部，本文档更新未重新构建或发布包。

## 后续平台实测的安全条件

1. 所有平台操作都在可丢弃的 fnOS 测试实例上进行，先做快照；数据使用专门构造的临时账户/夹具，不连接生产部署，不操作真实用户数据。
2. 证据需同时给出 fnOS 版本、FPK 版本和 SHA256、回调阶段与时间、执行用户、入口路径/权限、实际请求与状态码、关键日志及清理结果；敏感认证值必须脱敏。
3. 以用户已批准的 `os_min_version=1.2.0800` 为支持下限，旧 `1.0.0` 镜像缺失不再阻塞 G0；不得未经批准再次改变支持矩阵。**历史 2026-11-08 验收快照**记录了 1.1.1-3 A/B/C/D、连续 stop/start/start/reconnect、Socket fail-closed 与恢复、真实上游、SIGKILL 接管、remove 范围和同一 FPK 重装等证据；匿名/伪造页面/API/SSE 的平台 HTTP 200 `text/plain` `invalid token` 不构成授权通过。真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；UI logout 后旧 `ost` 会话仍可访问有效 Gateway page/status/SSE，未观察到服务端 session revoke；官方 PyPI 直连、完整生产路径/错误矩阵及生产发布门禁未完成。**截至 2026-11-09 当前状态：**G3 按已批准范围完成，G4 已实施并退役 CGI；当前 1.1.1-6 是测试候选，不是生产发布。手机复测仍待用户且不阻塞本轮验收；logout 平台现象未修复、会话未撤销。不能开放公开端口、跳过鉴权测试或退回 Shell 删除逻辑；不恢复 CGI fallback。官方 PyPI 直连 15 分钟无输出超时；阿里云真实 `qwenpaw==2.2.2.post1` 上游安装/卸载通过不等同官方源通过。历史原位降级被拒绝的响应只作为平台按设计禁止原位降级的证据，不得要求或建议再次尝试。
4. 表中按范围区分 G0 历史探针、当前真实平台部分通过项和剩余发布门禁。**历史状态（截至 2026-11-08）：**G0/G1 已退出，G2 本地实现与当时候选验收已完成；1.1.1-3 的 A/B/C/D、remove/重装和真实上游子项通过；真实手机 fnOS App 不纳入当时 G3 范围，UI logout 未观察到服务端 session revoke，真实失效会话、官方 PyPI 直连、完整生产路径/错误矩阵及其他发布门禁仍未完成，当时 G3 尚未整体关闭、G4 尚未开始。**当前状态（截至 2026-11-09）：**G3 按已批准范围完成，G4 已实施且生产 CGI 已退役；当前 1.1.1-6 为测试候选（路径和摘要见本文顶部），远端 `running` 不代表生产发布。真实手机复测仍待用户且不阻塞本轮验收；logout 后旧会话仍有效是已知平台问题，未称已撤销或已修复。官方 PyPI 直连 15 分钟无输出超时；阿里云真实 `qwenpaw==2.2.2.post1` 安装/卸载通过不等同官方源通过。不得把原位降级禁令证据、历史探针、静态源码、CI、文档或历史 FPK 校验扩写成生产发布、真实 PyPI 或 QwenPaw App 验收、全系统监听审计或 CGI 退役结果。

## 2026-11-04 G0 只读复核（目标版本 VM 实测前；历史，不代表 2026-11-07 当前状态）

- **环境证据：**当时检查的本机 `/etc/os-release` 显示 Debian GNU/Linux 12（bookworm）；当时仓库 `manifest` 声明 `os_min_version=1.0.0`。这是当时本机用户层与包声明，不是 fnOS 兼容性证据；不能据 Debian 12 判定 fnOS 不支持统一网关，也不能据此改变最低版本。
- **实例与 Socket：**当时没有可丢弃的 fnOS 最低版本/目标版本测试实例或可恢复快照。检查时本地候选 `gatewaySocket` 路径没有对应的运行时 Socket；这不证明 fnOS 平台不支持该能力。已有的 `supervisor-control.sock` 是私有控制 Socket，不是网关入口。
- **当时尚缺的证据：**没有真实网关 Socket 的路径、属主/权限、访问及生命周期记录；没有客户端伪造 `X-Trim-*` 覆盖/剥除、用户身份/授权、CSRF 或平台回调窗口证据。此段记录的是 VM 实测前的只读检查状态；最新结果见本文前部。
- **历史结论与后续收口：**该只读检查阶段 G0 未退出、G1 不得开始。后续已取得最低/目标版本 `1.2.0800` 的平台可信链及卸载 helper 执行窗口证据，并经用户批准调整支持下限，G0 现已退出；完整应用授权/CSRF、Socket 生命周期、正式升级、受支持回退和 App 验收仍保留为后续门禁，不能混为已通过。

## 2026-11-04 G0 隔离 VM 实测与完成结论（历史，不代表 2026-11-07 当前状态）

- **测试边界：**远程只读实例为 fnOS `1.2.0800`，仅查看，未在远程降级、安装或修改配置。安装和探针测试全部在本地全新 QEMU/KVM VM 完成，同为 fnOS `1.2.0800`，系统盘 24G、数据盘 20G（Btrfs Basic）。凭据不落入文档。
- **网关和路径：**最小探针以 `qwenpaw` 名称安装到 VM，配置 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock`。完整前缀 `/app/qwenpaw`、尾斜线、`/echo?case=admin` 均原样到达应用；仅为探针路径行为证据。
- **Socket peer：**Socket 为 `root:root`、mode `0755`。`SO_PEERCRED` 为 `/usr/trim/bin/trim_http_cgi`，采样时 PID `1151`、UID `0`、GID `0`。PID 是当时观测值，不代表稳定身份；未由此验证重启、清理及完整 Socket 生命周期。
- **会话身份：**管理员 `Yzy` UID `1000` 的 `isadmin=true`，普通用户 `g0user` UID `1001` 的 `isadmin=false`。客户端伪造 `X-Trim-Userid`、`X-Trim-Username`、`X-Trim-Isadmin` 均被覆盖为真实会话值。`allUsers=true` 时普通用户也可读取探针；这说明入口可读不等于业务授权，业务必须自行实施管理员授权。
- **未登录与跨站请求上下文：**未登录探针请求的实际响应为 HTTP `200`、body `invalid token`，表示探针应用返回了无效 token 响应，不是 HTTP 层拒绝或完整未登录授权测试结论。`Host` 被重写为 `127.0.0.1`；`Origin: https://attacker.invalid` 原样到达应用。跨站 Origin 到达应用本身不能证明变更请求会成功或被拒绝，应用仍须实施 CSRF 校验，不能只依赖 Host/Origin 相等。
- **安装与卸载执行窗口：**安装时 `target/bin` 可执行；`cmd/main` 应解析回调同级路径，而不是 `TRIM_APPDEST/cmd/main`。`appcenter start/stop` 可用。卸载回调时 target binary 已缺失，但 `TRIM_PKGVAR` 中的 helper 仍可执行，G0 原生执行窗口可行性已确认；不能再依赖 target binary。该观察不代表 helper 存活/清理协议、回调重试或完整 keep/remove 已验收。
- **升级/回退边界（截至当时）：**重复 `install-fpk` 返回 `installed`，但 `target/manifest` 仍是 `0.1.0`，未形成正式升级证据；之后仅为探针手工替换 binary，不得记作升级通过。正式旧包升级和真机回退在该时点均未验证；后续 2026-11-07 的正式 1.1.1-1 → 1.1.1-2 升级见本文前部，直接安装精确低版本时收到“已安装相同或更高版本”的原始响应，证明原位降级按设计被禁止，但不构成有效回退演练。受支持回退仍须先恢复未决事务并确认旧包兼容当前 schema/layout，再经官方保留数据的卸载流程卸载当前版、安装精确旧包、验证，最后再正式升级最终版；程序回退不等于用户数据回退。
- **最低版本与 G0 门禁结论（截至当时）：**旧 `1.0.0` 官方签名 URL 返回 `403`、公开 Quark 镜像要求登录且未下载/校验 MD5，保留为历史记录。用户已明确批准最低 `os_min_version=1.2.0800`，manifest 已更新；低于此版本的旧系统不再承诺 CGI 兼容支持。结合该最低/目标版本的网关、身份头覆盖、peer 及卸载 helper 可执行证据，G0 已完成并退出。G0 退出时 G1 尚属本地实现阶段；当前 G2/G3 证据见本文前部，不能以本历史段落宣称完整迁移或生产发布。
