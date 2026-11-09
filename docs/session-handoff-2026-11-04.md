# 会话交接：QwenPaw-for-fnOS 管理层整改

- 交接日期：2026-11-09（本文基于 2026-11-04 交接及后续历史记录更新）。
- 项目：仓库根目录。本文顶部为当前状态；后续按日期记录的 G0–G3 证据仍是历史事实，不覆盖最新候选状态。

## 当前状态

- G0–G3 按已批准范围完成；真实手机 fnOS App 暂不纳入本轮验收，最终手机复测仍待用户。G4 已完成，生产层为 Gateway-only：CGI 文件、生产 CGI 子命令、前端 action fallback、CGI 专属测试和 CI 门禁已删除。
- 管理控制台与 `qwenpaw.main` 均经 `/app/qwenpaw/web/` 使用同一 Gateway/Socket；不裸连内核端口，不使用 `target blank`。Rust `web_proxy` 适配上游路径、流式 SSE 和 Bearer Authorization，并分离管理系统 CSRF 与上游请求；WebSocket 返回 501，不宣传为完整支持。
- 当前测试候选 FPK：`build/candidates/1.1.1-11-dual-web-proxy-20261109/qwenpaw-1.1.1-11-x86.fpk`；SHA256 `2db563daa353fca8ac1ed6c59275375787eb1fae37abc722a49434a8098723c1`；supervisor SHA256 `b7813f1ab26315599035acfd042656c82634624c225005c1f0a36d1758bb3f6f`。远端为 `1.1.1-11 running`，内核 `2.3.0b2 running`，无活动事务；不代表生产发布。独立图标无port并走Gateway；独立TCP反代默认自动分配、冲突回退、前端可改且先bind再切换，当前端口33083。双入口真实Chromium中文鉴权、en保留、无30秒超时通过；gzip流式压缩生效。Rust 283+13、Clippy、Node 41、typecheck、build/audit通过，真实手机复测仍待用户。
- fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不声称会话已撤销或问题已修复。官方 PyPI 直连 15 分钟无输出超时；真实阿里云 `qwenpaw==2.2.2.post1` 上游安装/卸载通过，不等于官方 PyPI 直连通过。
- 前端合法 `seq + instance` 使用 `sessionStorage`，页面重建/pageshow 恢复同一任务轮询、不重复 POST，存储不可用时安全降级；语言仅在首次初始化时默认中文，不覆盖已有英文。
- 工作树中有本任务以前的未提交源码、CI、审计和删除变更及未跟踪文件；本轮仅同步指定文档，不覆盖既有内容。未改代码、未操作真实数据、未部署，也未 commit/push/tag/发布。
- 最终 helper 契约和历史候选证据保留在下文，作为历史记录，不表示生产 CGI 仍存在。

## 本地最终修复与验证

- `test-platform-callbacks` 33 项、卸载 Rust 15 项、严格 Clippy、preflight、`STRICT_CI`、build、audit 均通过；candidate audit 与 helper-first、keep deletion scope、真实 fnOS roots upgrade callback 回归均通过。
- `supervisor/src/install/process.rs` 已修复短命子进程登记竞态：仅同一身份已退出时允许 executable 缺失，存活或身份变化仍失败关闭。RED/GREEN 证据为 `/tmp/qwenpaw-g3-process-identity-20261107/`；install 87、runtime 5、严格 Clippy、user namespace 全 crate 276+13 通过。
- 本地 Gateway 回归在隔离 user namespace 使用 root UID 边界和合成管理员，只验证本地实现，不是 fnOS 身份验收；生产二进制保留仅由 `QWENPAW_ENABLE_CRASH_POINT=1` 与绝对路径 `QWENPAW_TEST_GATEWAY_PEER_EXE` 同时启用的非 root 测试 fallback，生产部署不得设置这两个变量，root 管理 peer 不走该 fallback。
- 2026-11-08 本地 WebKit engine 选择已加入 `web/playwright.config.ts`、`web/e2e/run.mjs`、`web/package.json`、`web/e2e/README.md`；隔离 Docker WebKit Gateway 2/2、Chromium Gateway 2/2 均通过，日志：`/tmp/qwenpaw-gateway-webkit-20261107/e2e-webkit-https.log`；Chromium 最终输出可写主控复验通过。WebKit 丢回执不是生产 beacon 失败：第三次 beacon 返回 `success=false`，复用同一 `seq/instance`，未新建事务；测试严格检查 `done`、`upgrading=false`、`recovery=false` 及 marker/pin/健康，不依赖可选 pip 日志，跨引擎 console 错误分别精确匹配。本地 WebKit/Chromium 继续作为浏览器兼容证据；真实手机 fnOS App 暂不纳入本轮 G3。
- 本地 Gateway 故障矩阵及 Docker `--user 0 --cap-add CHOWN` 目标回归保持既有通过结论：foreign-owner stale、reconnect、service readiness 3/3、0 跳过；证据 `/tmp/qwenpaw-g3-local-socket-20261107/`。真实上游冒烟已在同一实例以阿里云真实 `qwenpaw==2.2.2.post1` 完成；官方 PyPI seq1 因 15 分钟无输出超时失败且未重投。证据目录：`/tmp/qwenpaw-g3-real-upstream-20261107/`、`/tmp/qwenpaw-g3-real-upstream-20261108/kernel-uninstall-final-20261108.json`。
- 远端 1.1.1-3 A/B/C/D 及最终清理/重装证据见 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`，关键文件包括 `matrix-c-remaining.json`、`matrix-d-orphan-followup.json`、`matrix-http-auth-gate.json`、`final-cleanup-verification-20261108.json`；完整平台边界见 `docs/platform-validation-2026-11-03.md`。匿名/伪造页面/API/SSE 均为平台 HTTP 200 `text/plain` `invalid token`，不构成授权通过。真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；新 1.1.1-3 UI logout 证据（`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`）显示“我的账号→退出登录”导航至 `/login` 并重新登录成功，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE。该结果是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试；真实失效会话门禁仍未通过。

## 历史候选与验证（保留记录）

- G2 历史候选 SHA256 `c56922738f654dec48ad48b1f675f9de8fc00f4e0bc47f7b8983006c35a4d336`，大小 2,141,266 字节，包内 supervisor SHA256 `3b53b4c6307e87cbf6fc5ae16da3f62594c8739ada883165d88cd41add387df2`，test-runtime SHA256 `6d8a192872e65442e78875cae86b2f2fafcd06b21cfd054dae8c4ed510a2b88c`；不是当前候选。
- G1 历史候选 SHA256 `e3aa519a5907f2dc7fa96a04d7aef9cf4ee5271832e42fda2c2c72695ecad1f9`，大小 2,136,014 字节，包内 supervisor SHA256 `28df3174eff0f4b8b3882bb331bd680843441a5d4803690b787b92665f5b0e75`；该候选当时通过 Gateway 16/16、管理 API 与七点恢复，不是当前摘要。
- G1 前旧本地包（1.1.1 RC1）：`build/output/qwenpaw-1.1.1-1-x86.fpk`；大小 `2,071,727` 字节；SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`；旧包内 supervisor SHA256 `075ea6f48ab9c617aae9888eea63565e06ee5703e2afcdac5aa307697107c607`，旧测试 helper SHA256 `dd59f19e53204fa9f81d4cab097c0d89af002a95d595cc7247cfb84584521c91`。均为历史记录。
- 初审整改后的历史工作树通过：Rust 222+13、Node 30、严格 Clippy/typecheck、preflight、完整 STRICT_CI lint、audit、自测、API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 契约及 Chromium 1 项。历史包未另行解包重跑全部包内套件；历史 lint 结果与当前 G1 结果分别记录。
- 历史 `905e`/`59023c` 结果保留其原有限制；一次性 sibling 改名只用于基线准备，不能算正式升级通过。

本文档更新不改变实现或产物摘要；`python3 scripts/gen-audit.py --check` 与 `git diff --check` 在文档更新后复核。

## 外部/平台证据与未验证边界（历史快照截至 2026-11-08；当前状态以本文顶部为准）

G0 隔离 fnOS 探针已确认完整 `/app/qwenpaw` 前缀、客户端伪造 `X-Trim-*` 被真实会话值覆盖，以及 peer `/usr/trim/bin/trim_http_cgi`（UID `0` / GID `0`）。卸载回调时 target binary 已缺失，但 `TRIM_PKGVAR` helper 仍可执行，原生执行窗口可行性已确认。这些证据与已批准的支持下限使 G0 退出，但不替代完整应用验收；详情见 `platform-validation-2026-11-03.md` 和 `unified-gateway-migration-plan.md`。

截至 2026-11-08 的 G3 历史远端快照为 App Center `1.1.1-3 running`，Gateway/API HTTP 200，页面显示内核未安装；唯一 manager、无未决事务、无 `18088` listener，双 UDS 为 `0600`、UID/GID `994/989`。当时 1.1.1-2 → 1.1.1-3 经官方 update 单次升级，A/B/C/D、remove 范围和同一 FPK 重装恢复通过；匿名/伪造会话的页面/API/SSE 仅返回平台 HTTP 200 `text/plain` `invalid token`，不构成授权通过。历史直接原位安装精确旧包被平台以“已安装相同或更高版本”按设计拒绝，不是平台异常或有效回退演练；不得再次尝试。历史证据目录为 `/tmp/qwenpaw-g3-1.1.1-3-20261108`，受支持回退记录为 `/tmp/qwenpaw-g3-supported-rollback-20261107`；当前候选及状态见本文顶部。

### 2026-11-07新增真实 fnOS 生命周期证据

证据目录：`/tmp/qwenpaw-g3-lifecycle-20261107`。独立 TCP listener 核查未发现 QwenPaw TCP listener，两个预期 UDS 正常；这是服务/内核级归属证据，不是完整系统 FD 逐 PID 核验。

- 早期手工 root 恢复与手工 package 启动曾造成 App Center 状态不一致并出现 10330/10350；这些仅为试验过程记录，不计入正式恢复结果，也不据此认定当前包代码存在缺陷。随后按 package 身份正常停止测试现场，再经官方 App Center start 恢复基线。
- 历史 1.1.1-2 正式 SIGKILL 用例先精确核验 PID/starttime 459807/23000247，只发送一次 SIGKILL；立即且仅一次官方 App Center start/start 返回 HTTP 200/code 0。随后 App Center 为 running、Gateway HTTP 200，新 manager PID 462058；双 Socket、pidfiles、supervisor 二进制 SHA256 均核验正常，无内核进程和未决事务。该历史恢复子项通过；1.1.1-3 D 的新包 manager 接管见当前证据目录。
- 历史 1.1.1-2 正常 sudo reboot 自动接管：boot_id `2f296c6f-0e40-4a6d-956d-2d08fa27b96e`→`24485936-0f2d-40a9-8017-cd658dab4dc1`。平台于 18:05:51 记录 `APP_AUTO_STARTED`，`qwenpaw.console` 注册成功；未手工 start。最终 `1.1.1-2 running`，唯一 manager PID 2697/PPID 1，双 Socket 均为 0600、UID 994/GID 989 且由该进程监听，无孤儿内核进程或未决事务。Gateway page/status/install-status/install-log/logs 均 HTTP 200，install phase 为 idle、`recovery_required=false`。Gateway `installed=false/running=false` 仅表示 QwenPaw 内核未安装，不表示外壳未运行。
- 旧浏览器上下文的 401 和轮询超时是认证上下文问题，不计为平台失败；最终结果以全新认证上下文的只读复核为准。以上只关闭 SIGKILL 官方恢复及正常系统重启自动接管两个子项，不构成 App 验收或 G3 整体验收。

受支持回退链的真实 fnOS 1.2.0800 证据：

- `local-audit/manifest-and-layout-compare.txt` 证明 1.1.1-1 与 1.1.1-2 的 inner member set 相同，包内 supervisor SHA 均为 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`；入口、回调和布局相同，仅 manifest 版本/校验、HTML 版本显示及压缩物不同，无 schema/layout 变化。包 SHA 分别为旧包 `2ecbe8bdd8ab7b7710285791a403b33373a9d612040478ec4f1a47452abcf5a8` 与最终包 `667193f2053739a6263f0ddc125e5edf6a32ac05f11594c9be6c34ba5b06bf92`。
- `pre-uninstall-metadata.txt` 与 `old-version-remote-acceptance.txt` 的 `TRANSACTION_MARKERS` 为空，前置无未决事务。`official-uninstall.txt` 记录官方 UI 保留数据卸载 HTTP 200/code 0；`post-uninstall-remote-verification.txt` 证明 package target、target supervisor、var helper 与 app.sock 已移除，同时保留 `@appdata`、`@appconf`、`@apphome` 目录、共享配置 metadata 和 `supervisor-token` metadata。保留范围是路径/metadata 证据，未读取或校验共享文件内容；runtime 文件和 Socket 会随生命周期变化，不能据此声称整个 appdata metadata 不变。
- `old-version-official-acceptance.txt` 记录精确旧包安装后 App Center `1.1.1-1` `running`，Gateway 页面、status、kernel install status、logs 均 HTTP 200，`recovery_required=false`；`old-version-remote-acceptance.txt` 记录正确的 `/vol1/@appcenter/qwenpaw/app.sock` 与 `/vol1/@appdata/qwenpaw/supervisor-control.sock`、无 kernel markers 和无未决事务。旧版进程初始查询 `PID_COUNT=0` 后有 `PROCESS_EXACT_RETRY`；按 retry 结果 `PID_COUNT=1`，确认单一 supervisor，不能采用初始 0 作为最终结论。
- `official-final-update.txt` 记录官方 `update/task` 仅提交一次并返回 HTTP 200/code 0；`official-final-update-status-poll.txt` 记录终态 `status=5`。`final-version-official-acceptance.txt` 记录最终 `1.1.1-2` `running`、Gateway/API HTTP 200、`recovery_required=false`；`final-version-remote-acceptance.txt` 记录四处 supervisor/helper 路径均为预期 SHA `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`、两个正确 Socket 正常、无未决事务，最终数据/事务状态通过。

最终远端真实 fnOS 路径核验包括 `/vol1/@appcenter/qwenpaw/bin/qwenpaw-supervisor`、`/var/apps/qwenpaw/cmd/qwenpaw-supervisor`、`/var/apps/qwenpaw/var/qwenpaw-supervisor-helper`，以及保留数据中的 `/vol1/@appdata/qwenpaw/qwenpaw-supervisor-helper`，四者均为预期 SHA；旧 `/var/apps/qwenpaw/supervisor-control.sock` 只是错误检查路径。helper keep 保留 var helper，remove 仍只按既有 var 删除范围清理。callback trace 的 6 行属于既有 `/vol1/appcenter-downloads/...-tpk` 平台 trace，不与当前 667 FPK 的包身份混称。

仍未验证或仍受发布门禁约束：

- 真实手机 fnOS App 暂不纳入本轮 G3，不作为阻塞；本地 WebKit/Chromium Gateway 2/2 继续作为浏览器兼容证据，不扩展为手机 App 结论。
- 新 1.1.1-3 UI logout 证据（`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`）显示“我的账号→退出登录”导航至 `/login` 并重新登录成功，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE。该结果是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试；真实失效会话门禁仍未通过。匿名/伪造页面/API/SSE 拒绝仍为平台 HTTP 200 `text/plain` `invalid token`，不构成公开 HTML 或授权通过。
- 真实 fnOS 全量回调时序/重试、helper 全生命周期的所有权/执行窗口/清理和实际停止预算，以及超出 A/B/C/D 的完整平台错误矩阵仍受发布门禁约束；本地 helper 来源、0750、SHA、target 缺失及当前 remove/重装证据不能替代这些剩余门禁。
- 1.1.1-3 远端 C 已通过独立陈旧 `app.sock`、active listener、普通文件、软链接和 unknown owner 的 fail-closed 与恢复；B 已通过连续 stop/start/start/reconnect；D 已通过 manager SIGKILL→官方 start 接管、sentinel 存活、停止卸载和重装恢复。正常系统重启自动接管也已有真实证据。本地隔离 Gateway 矩阵仍作为补充，覆盖 stale/active socket、普通文件、软链接、权限拒绝、inode 替换防误删、孤儿内核接管和哨兵存活；这些结果不扩展为掉电持久性、失效会话或整体 G3 通过。
- 官方 PyPI seq1 因 15 分钟无输出超时失败且未重投；同一实例随后以阿里云真实 `qwenpaw==2.2.2.post1` seq2 done，import/metadata/pin/进程、18088 `/api/version`、18099 切换并恢复 18088、stop/start/stop 均通过。最终仅内核卸载发出一次 POST，HTTP 200 success；venv/pin/进程/listener 消失而 data root 保留，metadata 19→18 仅删除 `/vol1/@appdata/qwenpaw/qwenpaw-version`，root mtime 变化，不写全量不变。证据：`/tmp/qwenpaw-g3-real-upstream-20261107/`、`/tmp/qwenpaw-g3-real-upstream-20261108/kernel-uninstall-final-20261108.json`。
- 远端 CI、生产发布和完整平台/发布门禁仍未完成；`905e`/`59023c` 历史结果及一次性 sibling 改名基线准备均有原有限制，不能算升级通过。
- **截至 2026-11-08 的历史状态：**G4 尚未开始，CGI 当时仍保留。**2026-11-09 当前状态：**G4 已实施并退役 CGI；G3 按已批准范围完成。当前 1.1.1-6 为测试候选而非生产发布，详见本文顶部。

本地测试、历史候选和 G0/G3 局部平台证据不能提供完整平台专属矩阵、远端 CI 或发布事项的实际证据；真实手机 App 已按当前范围明确排除。

## 回退边界与操作状态

- fnOS 不允许直接原位降级安装 FPK，只能更新；历史阶段精确旧包的直接安装以“已安装相同或更高版本”被拒绝，原始响应仅作为原位降级禁令证据保留，不是平台异常，也不是有效回退演练，不得再次尝试原位降级。**截至 2026-11-08 的历史状态**为 1.1.1-3 已经官方 update 单次升级；**当前测试候选**为 1.1.1-6，远端经官方 update 后为 `running`，但不是生产发布。
- 历史受支持回退链（1.1.1-2/667 阶段）已在真实 fnOS 1.2.0800 完成并通过：前置无未决事务且确认旧/新包 schema/layout 兼容 → 官方保留数据卸载当前版 → 精确旧包 `2ecbe8bdd8ab7b7710285791a403b33373a9d612040478ec4f1a47452abcf5a8` 安装并验证为 `1.1.1-1 running` → Gateway/API、Socket、manager 单一性、数据/事务状态验证 → 官方 `update/task` 单次提交并以 `status=5` 回到当时最终 `1.1.1-2 running`。该历史回退子项已通过。**截至 2026-11-08** 的状态是 G3 尚未整体关闭、G4 尚未开始；**截至 2026-11-09**，G3 已按批准范围完成、G4 已实施并退役 CGI，具体当前状态见本文顶部。
- keep 证据只证明既定保留范围的目录/路径和 metadata；runtime 文件、Socket 与进程会随生命周期变化，不能表述为整个 appdata metadata 或共享文件内容不变。程序/包回退不等于用户数据回退，完整删除用户数据不可回滚。
- 未 commit、push、发布或部署；未操作真实用户数据。
