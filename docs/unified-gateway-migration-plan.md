# fnOS 统一网关迁移计划

- 当前状态（更新至 2026-11-09）：G0–G3 按已批准范围完成；真实手机 fnOS App 暂不纳入本轮验收，最终手机复测仍待用户。G4 已实施并完成 CGI 退役，当前为 Gateway-only：CGI 文件、生产 CGI 子命令、前端 action fallback、CGI 专属测试与 CI 门禁均已删除。管理控制台与 `qwenpaw.main` 均通过 `/app/qwenpaw/web/` 使用同一 Gateway/Socket，不裸连内核端口，不使用 `target blank`。Rust `web_proxy` 保留上游路径、流式 SSE 与 Bearer Authorization，并区分管理系统 CSRF 与上游请求。WebSocket 返回 501，不宣称完整支持。
- 当前测试候选（2026-11-10）：FPK `build/candidates/1.1.2-9-rc1-20261110/qwenpaw-1.1.2-9-x86.fpk`，SHA256 `8729939ab0f3adb8ee68575f26a32042a07abb3848d1812b911137868565e95a`；包内 musl 静态 supervisor SHA256 `1c56b64e76db11bb87ddc6cb5b1a2f817d0107a2347486adef6ed37d5ace1a21`。官方 7→8 与 8→9 各一次升级均成功；8→9 前后 venv/Python inode、版本钉和 pip 日志摘要不变，文件监控无 staging、venv 替换或安装写入，四份二进制一致。最终 `1.1.2-9 running`、内核 `2.3.0b2` 健康，public off、无管理员、loopback、无活动事务。证据 `/tmp/qwenpaw-1129-official-20261110/`。当前语义为 UNINSTALL+keep/unknown 删除 venv，只有官方 `TRIM_APP_STATUS=UPGRADE` 保留；remove 与仅卸载内核不变。Rust 317+13 与真实更新顺序测试通过。原生 restart、手机 App 与生产发布不在该结论内。已发布 RC1 与本地候选哈希不得混用。
- 前端合法 `seq + instance` 使用 `sessionStorage`；页面重建/pageshow 恢复同一任务轮询、不重复 POST，存储不可用时安全降级。语言只在首次初始化时默认中文，不覆盖已有英文。
- 下文 G0–G3 记录和旧版 G4 计划均为截至 2026-11-08 的阶段历史，不能覆盖本状态或作为当前待办；保留以供审计。历史直接原位降级被拒绝的记录不改变现行受支持回退步骤。文档更新不执行平台操作。
- 范围：当前已将管理控制台和 QwenPaw 主 Web 入口统一接入 fnOS Gateway；QwenPaw 本体与管理核心保持独立，代理只处理 Web 路径，不把内核端口直接暴露给浏览器。

## G0 只读核查记录（2026-11-04，VM 实测前）

- 当时本机 `/etc/os-release` 标识为 Debian GNU/Linux 12；仓库 `manifest` 的 `os_min_version` 声明为 `1.0.0`。本机用户层识别为 Debian 12 不能证明目标 fnOS 的兼容性，也不能据此判定 fnOS 1.0.0 不支持统一网关。
- 当时没有可丢弃的 fnOS 最低版本/目标版本测试实例或可恢复快照；没有真实 fnOS Gateway Socket、网关请求头覆盖、用户角色/管理员授权、CSRF 或生命周期回调窗口的实测证据。
- 当时本机未发现候选 Gateway Socket 路径对应的运行时 Socket。缺少本地 Socket 只说明当时未观察到该 Socket 实例，不证明 fnOS 平台不支持 `gatewaySocket`；私有 `supervisor-control.sock` 也不是该网关入口。
- **历史结论：**该只读核查阶段 G0 未退出，G1 不得开始；当时仅做只读核查和文档记录，没有实现 Gateway Listener、切换前端或入口、退役 CGI、运行代码测试或执行平台回调/真实数据操作。
- **历史门禁与后续收口：**当时最低版本仍为 `1.0.0`，因此要求补齐最低/目标版本及回调窗口证据。现已按用户批准调整支持下限，并以网关可信链和卸载 helper 执行窗口证据完成 G0；完整管理 API 授权/CSRF、Socket 生命周期、正式升级、受支持回退及 App 验收仍是后续阶段门禁，不据探针结果预称通过。

## G0 隔离 VM 实测与完成记录（2026-11-04）

- **历史测试边界（2026-11-04）：**当时远程只读实例为 fnOS `1.2.0800`，仅查看，没有在远程实例降级、安装或修改配置。所有安装和探针请求均在本地全新 QEMU/KVM VM 完成，系统为同版 fnOS `1.2.0800`，系统盘 24G、数据盘 20G（Btrfs Basic）。之后获授权的专用远端测试实例已用于本轮候选安装、卸载、升级与直接原位安装尝试；其当前状态和边界见本文状态摘要及 G3 记录。本地 VM 的隔离边界不变。凭据不落入文档。
- **网关与路径：**以 `qwenpaw` 名称安装最小探针，配置 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock`。完整前缀 `/app/qwenpaw`、尾斜线以及 `/echo?case=admin` 均原样到达应用；这是最低/目标版本探针的路由证据，不代表完整控制台已接入。
- **Socket 与 peer：**探针 Socket 为 `root:root`、mode `0755`。`SO_PEERCRED` 观察到 `/usr/trim/bin/trim_http_cgi`，当时 PID `1151`、UID `0`、GID `0`；这是采样时的 peer 凭据，不是稳定 PID 或完整 Socket 生命周期证明，探针 mode 也不是最终安全配置。
- **身份头与用户：**管理员 `Yzy`（UID `1000`）到达应用时为 `isadmin=true`；普通用户 `g0user`（UID `1001`）为 `isadmin=false`。客户端伪造的 `X-Trim-Userid`、`X-Trim-Username`、`X-Trim-Isadmin` 均被覆盖为真实会话值。普通用户在 `allUsers=true` 时也能读取探针，说明入口可达性不等于业务授权；真实管理业务必须自行实施管理员授权。
- **未登录、Host、Origin 与 CSRF（历史探针观察）：**未登录探针请求到达应用，响应为 HTTP `200`、body `invalid token`；这表示本次探针观察到的是应用层无效 token 响应，而非 HTTP 层拒绝，不能当作完整未登录授权矩阵通过。`Host` 被改写为 `127.0.0.1`，`Origin: https://attacker.invalid` 原样到达应用；跨站 Origin 能到达应用不证明跨站变更已被接受或拒绝，应用仍必须做 CSRF 校验，不能只依赖 Host/Origin 相等。
- **安装与回调窗口（历史探针观察）：**安装时 `target/bin` 可执行；`cmd/main` 应从回调同级路径解析，不能假定为 `TRIM_APPDEST/cmd/main`。`appcenter start/stop` 可用。卸载回调实测确认 target binary 已缺失，但 `TRIM_PKGVAR` 中的 helper 仍可执行；G0 原生执行窗口阻塞已解除，后续实现不能依赖卸载时 target binary 仍存在。helper 来源/权限、存活/清理、重试幂等及完整 keep/remove 行为仍须验证。
- **升级边界（截至 2026-11-04）：**重复 `install-fpk` 虽返回 `installed`，但 `target/manifest` 仍为 `0.1.0`，不构成正式升级证据；随后仅为探针手工替换 binary，不能记作升级通过。正式旧包升级与受支持回退在该时点均未验证；2026-11-07 的正式 1.1.1-1 → 1.1.1-2 升级及直接原位安装低版本时收到的原始拒绝响应见本文当前 G3 记录。该响应证明原位降级按设计被禁止，不是平台异常或有效回退演练。
- **最低版本与 G0 完成结论（截至 2026-11-04）：**旧 `1.0.0` 官方签名 URL 返回 `403`、公开 Quark 镜像需登录且未下载/校验 MD5，保留为历史缺口。用户已明确批准最低 `os_min_version=1.2.0800`，manifest 已更新；低于此版本的旧系统不再承诺 CGI 兼容支持，不维护旧系统 CGI 兼容发布路径。最低与目标版本现为同一已实测版本，结合网关完整前缀、身份头覆盖、peer 及卸载 helper 可执行证据，G0 已完成并退出。G0 退出时 G1 尚属本地实现阶段；当前 G2/G3 证据见本文后部，不能以这段历史结论宣称完整迁移、CGI 退役或生产发布。

## 1. 目标与当前判断

### 目标架构

```text
浏览器 / fnOS App
      │
      ▼
fnOS 统一网关：登录会话校验 + 稳定 URL 前缀
      │ gatewaySocket（网关入口，独立于内部控制 Socket）
      ▼
Rust supervisor：静态控制台 + 管理 HTTP/API + 会话/CSRF校验
      │
      ├── ManagementCore（操作锁、事务、恢复、授权策略）
      ├── supervisor-control.sock（私有 CLI/平台控制，保留）
      └── RuntimeAdapter → 外部受管 QwenPaw 内核
```

建议入口：`/app/qwenpaw`。最终桌面配置使用 fnOS 支持的 `gatewayPrefix` 与 `gatewaySocket`，并明确图标/桌面入口路由到该前缀。**历史迁移设计（截至 2026-11-08）：**CGI 曾计划仅在迁移期保留，验收后退役；截至 2026-11-09，G4 已实施，生产 CGI 已从入口配置、FPK 物料及生产回归中退役，当前为 Gateway-only。

`qwenpaw.main` 与运维控制台均经 `/app/qwenpaw/web/` 走统一网关。当前产品路径不让浏览器直连内核端口，也不使用 `target blank`。

### 当前结构的正误边界

当前结构：

- Rust supervisor 是常驻管理服务，QwenPaw 是它管理的独立子进程。内核未安装、停止或启动失败，不应导致 supervisor 一并退出。
- `ManagementCore` 被 Gateway、管理 HTTP 和平台私有控制通道复用，安装/配置/恢复逻辑不复制。
- `supervisor-control.sock` 是独立私有控制面；它不应被浏览器访问，也不是 fnOS `gatewaySocket`。
- Gateway `/web/` 路由由 Rust `web_proxy` 适配上游路径、流式 SSE 与 Bearer Authorization；管理系统 CSRF 与上游请求分离。WebSocket 返回 501。
- `app/ui/config` 同时为 `qwenpaw.console` 和 `qwenpaw.main` 配置 `/app/qwenpaw` 与 `app.sock`；两入口共享同一 Gateway/Socket。生产 CGI、CGI 子命令及旧前端 action fallback 已删除。
- G3 按用户批准的范围完成；真实手机 fnOS App 暂不纳入本轮，最终手机复测待用户。fnOS logout 后旧会话仍有效属于已知平台问题，用户确认不阻塞应用验收，不能声称会话已撤销或已修复。

候选和生产发布是不同状态：当前 1.1.1-6 测试候选经官方 update 后远端为 `running`，无内核、无未决事务，但不代表生产发布。

## 2. 设计原则与不变量

1. **不改业务核心**：Gateway Router、管理 HTTP 与私有控制 UDS 都只作为传输层调用同一个 `ManagementCore`；生产 CGI 已退役，不存在 CGI 兼容 Router；不复制安装/配置策略。
2. **入口 Socket 与私有 Socket 严格分离**：不同路径、用途、访问主体、权限和请求格式。
3. **网关提供登录态，不自动等于管理员授权**：所有管理 API 明确实施角色检查。建议控制台及安装/卸载/日志/认证操作默认只允许管理员；是否支持普通用户只读页需单独定义最小白名单。
4. **不信任未验证的请求头**：只有确认来自 fnOS gateway 的实际连接才可使用平台身份头；必须实测客户端提交同名 `X-Trim-*` 时网关是覆盖还是剥除。不能仅凭 Header 内容识别“这是网关”。
5. **网络最小暴露**：不新增公开 TCP 管理端口，不取消当前 loopback 限制，不把 token 放 URL，也不向浏览器泄露 `supervisor-token`。
6. **CSRF 保持有效**：迁移不能因为“已有网关登录”就取消 CSRF。保留安全的浏览器 CSRF 令牌/自定义 Header 机制；Origin/Host 策略需能承受反向代理 Host 重写。
7. **管理器与被管内核生命周期分离**：内核停止时控制台、状态、安装和恢复 API 仍可用；管理器自身停止/崩溃时 Web 不可用是可接受边界，由应用中心和 `cmd/main service start` 恢复。不能宣称管理器停机时网页还能自救。
8. **明确 readiness**：平台 start 成功必须确认 supervisor 已完成恢复并且 gatewaySocket 已可服务；PID 文件存在或 loopback API 可达不能单独作为网关可用证据。
9. **失败关闭并保留受控回退**：预期启用网关但 Socket 无法安全绑定时不能静默返回“已就绪”，必须返回明确 degraded/失败状态并由平台恢复路径处理。**历史迁移期设计（截至 2026-11-08）：**入口切换前曾计划保留 CGI；当前 G4 已实施、生产 CGI 已退役，不再保留 CGI fallback。若未来需回退，必须另行授权并验证完整的入口注册、静态物料与服务配置恢复，不得隐式恢复 CGI。
10. **不修改用户数据 schema**：该阶段不改变 venv、版本钉、settings/install journal；有未决事务先按旧版协议恢复，不能把平台迁移与持久化格式升级绑在一起。

## 3. 关键设计决策（实施前通过 P0 验证）

### 3.1 Socket 路径、属主与访问控制

- fnOS `gatewaySocket` 需按平台规定使用 Socket 文件名，并放在安装后的 `TRIM_APPDEST`/target 所要求的位置；建议采用独立的 `qwenpaw-gateway.sock`，而不是 `supervisor-control.sock`。
- 确认 supervisor 的实际 UID/GID、目标目录是否可写、网关工作进程 UID/GID，以及 fnOS 如何访问 Socket。不要根据 DSH 的 `0660` 或当前私有 Socket `0600` 猜权限。
- 采用满足“只有平台网关能连接”的最小 mode/group/ACL；若 OS 提供可靠的 peer credentials，记录并校验 UID/GID。`SO_PEERCRED` 只证明进程凭据，不自动证明某个 HTTP Header 是平台可信信息。
- 处理陈旧 Socket：必须先确认文件类型、属主、inode 和监听者；普通文件、软链接、未知属主或活跃 Socket 均拒绝覆盖/删除。SIGKILL/重启后需能安全恢复。
- supervisor 要持有 Listener 至整个服务生命周期；优雅关闭后清理自己创建的 Socket。对比服务进程更替、平台升级与路径 inode，不能误删另一实例的 Socket。

### 3.2 路由与前缀

- 浏览器唯一控制台前缀：`/app/qwenpaw`；保持静态资源和 API 在同一源和前缀下。前端不得硬编码 `index.cgi/api.cgi`。
- G0 隔离探针已验证网关完整保留 `/app/qwenpaw` 前缀；未验证前缀剥离形态。G1 只能以已证实的完整前缀为依据，不得将未观察到的另一形态写成已实测；如需兼容另一形态，须有证据并避免双重剥离。
- 覆盖 `/app/qwenpaw`、尾斜线、查询参数、编码路径、API、错误路由和 SPA fallback；仅静态白名单可访问，不得暴露日志目录、`var`、token、journal 或其他 app 目录。
- 评估 `GET/HEAD`、压缩、ETag/cache、SSE/长轮询和 WebSocket。只实现当前控制台实际需要的流式协议，不因 DSH 有 WebSocket 而照搬其终端反代。
- 页面前缀切换、缓存失效和旧 URL 重定向策略须可逆；不能让旧 CGI 与新前缀同时接收有副作用的同一操作而造成双提交。

### 3.3 身份、管理员权限与 CSRF

身份认证分两层：

1. 网关确认用户已登录，并向后端提供可信的用户上下文。
2. QwenPaw 管理服务判断该用户是否有权执行特定管理操作。

实施要求：

- P0 用真实 fnOS 证明 `X-Trim-Userid`、`X-Trim-Username`、`X-Trim-Isadmin` 的值、缺失语义、覆盖规则及不同用户角色；负例包括未登录、普通用户、管理员、伪造客户端头和过期会话。
- API 的角色矩阵覆盖查看状态、日志、启动/停止、改端口、改镜像、安装/卸载内核、重置认证。不能因桌面入口隐藏而省略后端授权。
- Header 只有在网关专属 Unix Listener/已验证的可信 peer 上才可采信；loopback TCP、CLI 私有 Socket或测试直连不能伪装为网关请求。需要本地 admin CLI 时走既有 peer-credential private control 路径。
- `allUsers=false` 是入口显示/访问配置，不等于每个服务端路由都有正确管理员角色检查。
- 保留 CSRF：POST/DELETE 等所有变更操作需要同源安全凭据。CSRF token 由 Rust 签发/校验，不能把管理 token 发给 JS。明确 cookie 的 `Path`、`SameSite`、HTTPS 下 `Secure` 行为；`__Host-` cookie 的 path/domain 约束要与前缀方案匹配。
- 变更请求若带 `Origin`，只接受唯一且合法的 `http`/`https` origin；`Sec-Fetch-Site: cross-site` 继续直接拒绝。优先将它与唯一合法的 `X-Forwarded-Host` + `X-Forwarded-Proto` 组成的外部 origin 比较，以兼容 fnOS 将 `Host` 改写为 `127.0.0.1`；无 `X-Forwarded-Host` 时，要求唯一合法 `Referer` 的 origin 与 `Origin` 一致，`Referer` 缺失、非法或重复均拒绝。不得简单比较 `Origin == Host`，Origin 校验只是补充信号，HMAC 签名/double-submit CSRF 仍是主防线；拒绝日志固定为 `invalid origin` 且不记录来源值。CORS 默认不开放。

若无法证明网关覆盖客户端同名身份头，或不能识别真实网关调用者，则不得仅信任 `X-Trim-*`；相关管理请求必须 fail closed，并解决可信身份边界后再开放。生产 CGI 已退役，不得将其作为替代授权路径或回退；不使用来源 IP 猜用户身份。

### 3.4 HTTP 服务装配与错误隔离

- 共用的路由/静态服务层分别装配 Gateway UDS 和现存 loopback HTTP；复用状态、限制 body、错误映射和管理核心。
- Gateway listener 生命周期纳入 supervisor 启动状态：路径创建/权限检查、bind、可执行健康请求均成功后才报告 gateway-ready。
- private control UDS 保持独立身份模型，不接受浏览器身份，也不能给其调用注入伪造的网关管理员身份。
- 内核不存在、未安装、启动失败和事务 recovery-required 时，gateway 仍可提供控制台、状态、日志和恢复提示；恢复门禁照常阻止危险写操作。
- Gateway listener 失败时记录明确原因和日志，不应误报平台 start 成功；交由应用中心恢复路径处理，不保留 CGI 静态入口作为隐式回退。

## 4. 分阶段改造

每阶段应单独审查并保留可回退提交边界；本计划不预授权 commit、push、部署或删除真实数据。

### G0：平台门禁与契约冻结（已完成并退出，不改生产入口）

已完成的平台可行性与支持范围决定：

- 用户已明确批准最低 `os_min_version=1.2.0800`，manifest 已更新；最低与目标版本相同。旧 `1.0.0` 镜像未取得不再阻塞 G0，也不据此宣称旧系统不具备网关能力；低于 `1.2.0800` 的旧系统不再承诺 CGI 兼容支持，不维护单独的旧系统 CGI 兼容发布路径。
- 隔离 fnOS `1.2.0800` 探针证明 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock` 可用，完整前缀、尾斜线和查询串原样到达；独立 Gateway UDS 实现应以此契约为依据。
- 网关覆盖客户端同名 `X-Trim-Userid`/`Username`/`Isadmin`；peer 为 `/usr/trim/bin/trim_http_cgi`（UID `0` / GID `0`）。采样 PID 和探针 Socket mode 不作为永久身份或最终权限配置。未登录/普通用户探针响应不等于完整业务授权或 CSRF 已通过。
- 安装时 `target/bin` 可执行，`cmd/main` 从回调同级路径解析，`appcenter start/stop` 可用；卸载回调时 target binary 缺失，但 `TRIM_PKGVAR` helper 可执行，原生执行窗口可行性已确认，不能再依赖卸载时 target binary 仍存在。

**历史 G0 交付与退出结论（截至 2026-11-04）：**平台路径/可信链、卸载执行窗口与支持版本决定已记录，G0 已退出，可实施 G1；当时这不是 CGI 退役 go，CGI 主入口和物料继续保留。**当前状态（2026-11-09）：**G4 已实施，生产 CGI 已退役；受支持旧包回退路径及已知 SHA 的旧包已在 G3 取得真实验证。

**历史验收边界（截至 2026-11-08）：**G0 探针和卸载执行窗口结论是历史/阶段证据；前序完整应用授权/CSRF、桌面 Gateway 与 Socket stop/start，以及 1.1.1-3 正式安装/升级、A/B/C/D、remove 范围、同一 FPK 重装、受支持回退、SIGKILL 接管和上游冒烟已有对应范围的真实验收。真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；本地 WebKit/Chromium 仅为浏览器兼容证据。UI logout 后旧 `ost` cookie 仍可访问有效 Gateway page/status/SSE，未观察到服务端 session revoke；官方 PyPI 直连、完整生产路径/错误矩阵和未覆盖的回调失败/重试仍是生产发布门禁。**当前状态（2026-11-09）：**G3 按已批准范围完成，G4 已实施并退役 CGI；不把上述生产发布门禁倒推为 G3 未完成，也不将其写成 CGI 退役尚未完成。手机复测仍待用户且不阻塞当前验收；logout 平台现象未修复，不声称会话已撤销。直接原位降级被拒绝的响应不是受支持回退链的替代证据；不得用重复 `install-fpk`、直接原位安装低版本或手工换 binary 代替正式升级/回退。

### G1：新增 Rust 网关 Listener 与路由（本地实现及源码/黑盒回归已完成）

本地完成范围：新增独立 Gateway UDS，私有 `supervisor-control.sock` 继续独立；CGI 主入口和前端未切换，CGI 未退役。`QWENPAW_GATEWAY_SOCKET` 只在测试或显式启用本地候选时设置，不代表 fnOS 已接入。

本地实现与回归证据（隔离本机，不是 fnOS 完整应用验收）：

- Gateway UDS 与私有 control UDS 分离；请求只接受验证过的网关 peer 和管理员角色。对变更请求使用 HMAC 签名的 double-submit CSRF；回归覆盖伪造/缺失身份、非管理员及无效 CSRF 拒绝。
- 网关与 CGI 静态资源复用 `web_static`；Socket 绑定后固定本实例 inode 身份，替换后拒绝并防止误删非本实例 Socket。
- SSE 并发配额为 8，总连接上限为 32；超限返回 HTTP 503 和 `Retry-After: 1`。service readiness 验证实际 Gateway UDS、HTTP 响应及 supervisor 身份，不以 PID/loopback 代替。
- 实际通过：两轮默认并行 Rust 262+13、严格 Clippy、release binaries 构建、Gateway UDS 黑盒 16、native API 15、index 22、管理边界、preflight 与 audit；`STRICT_CI=1 bash scripts/lint.sh` 全部通过，包括 120 次生命周期并发与配置回调。
- `build.sh` 和 `audit-fpk` 均通过。新 G1 候选 `build/output/qwenpaw-1.1.1-1-x86.fpk` 大小 2,136,014 字节，SHA256 `e3aa519a5907f2dc7fa96a04d7aef9cf4ee5271832e42fda2c2c72695ecad1f9`；包内 supervisor SHA256 `28df3174eff0f4b8b3882bb331bd680843441a5d4803690b787b92665f5b0e75`，test-runtime SHA256 `6d8a192872e65442e78875cae86b2f2fafcd06b21cfd054dae8c4ed510a2b88c`。解包包内 supervisor 后 Gateway 16/16、`test-supervisor-api` 和七点 SIGKILL recovery 均通过。

本地 G1 实现/源码/候选包回归已完成；fnOS 完整应用接入、桌面/前端切换与平台验收仍待后续阶段。不能把本地隔离 user namespace 的 root UID 夹具或显式 UDS 环境变量写成真实 fnOS 网关鉴权证据；生产二进制保留仅由 `QWENPAW_ENABLE_CRASH_POINT=1` 与绝对路径 `QWENPAW_TEST_GATEWAY_PEER_EXE` 同时启用的非 root 测试 fallback，生产部署不得设置这两个变量，root 管理 peer 不走该 fallback。

### G2：前端协议切换与 fnOS 测试候选（本地候选完成；平台验收部分完成）

本地已完成的实现与验证：

- **历史 G2 实现（截至 2026-11-08）：**`web/src/api.ts` 曾按页面路径选择 Gateway REST `/app/qwenpaw/api` 或迁移期 CGI action 协议；此双协议实现已在 G4 移除。当前仅使用 Gateway REST，管理操作复用同一核心。
- 安装网络失败使用 Gateway JSON beacon 和 CSRF query，不跨协议自动重放；测试确认丢失受理响应后只恢复同一任务，不重复安装。
- `app/ui/config` 当前控制台为 `/app/qwenpaw/`，同条目配置 `gatewayPrefix=/app/qwenpaw`、`gatewaySocket=app.sock`；`cmd/main` 固定导出 `${TRIM_APPDEST}/app.sock`，`qwenpaw.console` 与 `qwenpaw.main` 共用 Gateway/Socket。CGI 文件曾在 G2 候选中作为回退物料，截至 2026-11-09 已在 G4 删除。
- 历史 G2 增加的 `scripts/browser-gateway.py` 与 `web/e2e/gateway.spec.ts` 曾以测试 HTTP 适配器覆盖 Gateway 管理流程；隔离 user namespace root UID 边界与合成管理员只验证本地行为，不代表 fnOS 身份验收。CGI E2E 原测试和对应 CI/Release 门禁均已于 G4 删除；Gateway 回归仍保留。
- 主控实际通过 Rust 264+13、Node 35、严格 Clippy/typecheck/build、Gateway UDS 17、Chromium Gateway 2 与 CGI 1、完整 STRICT_CI lint、build/audit/preflight。G2 历史候选 `build/output/qwenpaw-1.1.1-1-x86.fpk` 大小 2,141,266 字节，SHA256 `c56922738f654dec48ad48b1f675f9de8fc00f4e0bc47f7b8983006c35a4d336`；包内 supervisor SHA256 `3b53b4c6307e87cbf6fc5ae16da3f62594c8739ada883165d88cd41add387df2`，解包后 Gateway 17、管理 API 与七点 SIGKILL recovery 均通过。该包是本地 G2 历史候选，不是当前远端最终包。

**截至 2026-11-08 的 G2/G3 历史证据摘要：**前序真实平台验收已通过完整应用管理员、普通用户、客户端伪造 `X-Trim-*` 头与 CSRF 矩阵，以及桌面 Gateway 与 Gateway Socket stop/start；1.1.1-3 的 A/B/C/D、完整受支持旧包回退、SIGKILL 后官方恢复、正常系统重启自动接管、限定范围 TCP listener、remove/重装及隔离 Docker WebKit/Chromium Gateway 2/2 子项按记录范围通过。真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；UI logout 后旧 `ost` cookie 仍有效，未观察到服务端 session revoke。官方 PyPI 直连、完整生产路径/错误矩阵和其他生产发布门禁仍未全部完成。真实阿里云 `qwenpaw==2.2.2.post1` 上游冒烟已完成，WebKit/Chromium 是浏览器兼容证据。**截至 2026-11-09 的当前状态：**G3 按批准范围完成，G4 已实施并退役 CGI；手机复测仍待用户且不阻塞本轮验收，logout 平台现象未修复。

## 历史快照：2026-11-08 G3 当时验收（A/B/C/D 通过；当时整体尚未退出）

以下状态仅记录截至 2026-11-08 的验收结论；不得覆盖本文顶部 2026-11-09 当前状态。

- 当时最终候选：`build/candidates/1.1.1-3-gateway-g3-20261108/qwenpaw-1.1.1-3-x86.fpk`，FPK SHA256 `94679e4d8343c1bf341e8a1cc2a1451a9ad0ad3296d2fc668deab456db601b51`；包内 supervisor SHA256 `2829f1a4079139f166f78478de804d3f619226d646ea93d58d0965718dbc4400`。该包已从 1.1.1-2 经官方 update 单次升级并在远端验收；1.1.1-2/c571 为历史本地候选，667 为更早远端包。
- [x] A 完整 GET/HEAD/query/404/编码拒绝/Content-Type/SSE 断连矩阵通过；B 官方连续 `stop/start/start/reconnect` 通过。
- [x] C 真实 `app.sock` 的 stale、active listener、普通文件、软链接和 unknown owner 均按 fail-closed 处理并恢复；D 阿里云真实 `qwenpaw==2.2.2.post1` 安装、健康、stop/start/stop、仅卸载通过。
- [x] D 还通过新包 manager 一次 SIGKILL → 一次官方 start → 原内核 PID/starttime/exe 接管、sentinel 存活，随后停止并卸载；remove 范围和同一 FPK 官方重装恢复通过。
- [x] 最终 App Center 为 `1.1.1-3 running`，唯一 manager、Gateway HTTP 200、无 QwenPaw 内核/未决事务/`18088` listener，双 UDS mode `0600`、UID/GID `994/989`。匿名/伪造会话的页面/API/SSE 均为平台 HTTP 200 `text/plain` `invalid token`，不构成公开 HTML 或授权通过。证据总目录：`/tmp/qwenpaw-g3-1.1.1-3-20261108/`。
- [ ] 真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞。新 1.1.1-3 UI logout 已导航至 `/login` 并重新登录，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；真实失效会话门禁仍未通过。官方 PyPI 直连、完整生产路径/错误矩阵及最终发布门禁仍未完成；截至 2026-11-08 的历史结论是当时 G3 尚未整体关闭、G4 尚未开始。
- 最终 callback 为 var helper 优先、sibling 回退、缺二者返回 `127`：先检查可执行的 `${TRIM_PKGVAR}/qwenpaw-supervisor-helper`，再检查回调同级 `qwenpaw-supervisor`。upgrade_callback SHA256 `b6cb930af324cf7d255ccfcc1c6c75e55c432017c5babb2f180507f542be130a`，uninstall_callback SHA256 `c6cdc62e7056dd176c35f8f9317af7069f03a14e0b566354fccd63d57dba947f`；candidate-audit 与 test-platform-callbacks 均为 `ALL CHECKS PASSED`。真实 fnOS `@apphome`、`@appconf`、`@appdata` 路径校验已修复并纳入候选。
- 历史 `905e` 与 `59023c` 原位升级限制不计入当前通过：升级时 callback 由已安装包提供，不能假设新包 callback 已生效；`59023c` 的 sibling-first 与真实路径校验导致原位升级失败。为重建测试基线，曾仅将 root 所有的旧 sibling 原子改名以强制 var helper，再正式执行 keep 卸载/安装；这是测试 fixture 准备，不是历史原位升级通过。
- 历史阶段（1.1.1-2）：helper-first 1.1.1-1 已通过正式 UI 安装并 `running`；随后 1.1.1-1 → 1.1.1-2 正式 UI 升级已通过，update/task 与 update/status 均 HTTP 200、code 0，状态为 `upgrading → starting → running`。向导默认“阿里云镜像源（推荐 - 兼容公网与云服务器）”；该阶段只更新飞牛外壳，不改变已安装 QwenPaw 内核版本。截至 2026-11-08 的 1.1.1-3 已由官方 update 单次升级完成。
- keep 卸载已通过正式 UI。共享根 exact metadata pre/final 均 1,119 行、171,041 字节、SHA256 `e10819ba4e6bbc230a2474ff11dea0b8a0797e8a9ee209e02764242f0643ff75`，added=0、removed=0、changed=0（比较 path/type/mode/owner/size/mtime）；凭据 metadata 记录数前后均 1、`contentRead=false`，`supervisor-token` 卸载后仍存在。appdata runtime metadata 18 → 16 行，移除 2 条，因此不能宣称整个 appdata metadata 完全不变。
- 历史 667 远端状态（1.1.1-2）：App Center installed HTTP 200、code 0、版本 1.1.1-2、`running`；download HTTP 200、code 10030；Gateway status/logs 均 HTTP 200，badge“内核未安装”、version“未安装”，不含“状态不可达”，三个服务按钮均 hidden/disabled。最终 manager PID 395874；kernel markers `/var/apps/qwenpaw/var/venv`、`qwenpaw-venv`、`qwenpaw-version`、`qwenpaw-version-pin` 均 absent。截至 2026-11-08 的 1.1.1-3 最终远端状态见本节当前状态和证据目录。
- 历史 1.1.1-2 observer 的不刷新 panel stop/start 已通过：stop 后 App Center 为 `stopped`，observer 显示“状态不可达/无法获取”，status/logs 404，manager 与 `app.sock` 消失；start 后同一 observer 恢复 HTTP 200 并显示“内核未安装”，App Center 回到 1.1.1-2 `running`，manager 与 socket 恢复。独立 start/check 证据也为 HTTP 200、code 0，顶层 events 为空仅是 observer 回调未回填的小瑕疵；1.1.1-3 的连续 stop/start/start/reconnect 见当前 A/B 证据。
- 历史 1.1.1-2 二进制/Socket metadata：manager 仅 1 个进程，PID 395874，`/proc` exe 为 `/vol1/@appcenter/qwenpaw/bin/qwenpaw-supervisor`。该路径 mode 775、`/var/apps/qwenpaw/cmd/qwenpaw-supervisor` mode 755、`/var/apps/qwenpaw/var/qwenpaw-supervisor-helper` mode 750，三者均 4,435,352 字节且 SHA256 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`；`/vol1/@appcenter/qwenpaw/app.sock` mode 600、inode 103；`/vol1/@appdata/qwenpaw/supervisor-control.sock` mode 600、inode 89。`/var/apps/qwenpaw/supervisor-control.sock` 缺失只是旧/错误路径，不能据此声称正确 control socket 缺失。final logs metadata：`supervisor.log` 30,167 字节、`qwenpaw.log` 0、`qwenpaw-config.log` 2,733、`qwenpaw-callback-trace.log` 7,102、`qwenpaw-upgrade.log.pip` 0；1.1.1-3 的最终独立 Socket/身份证据见当前证据目录。
- 历史 1.1.1-2 `/vol1/@appdata/qwenpaw/pip-mirror` metadata：pre/final 均 mode 750、uid/gid `994/989`、大小 40 字节、SHA256 `5e2868300962c78938530abb33c3c3d7a724086a6f4a661ad369ce87487945f`，final inode 84；未输出正文。shared-root exact metadata 只证明 metadata 一致，未读取或哈希共享根内容，不能扩写为内容一致。
- 历史受支持回退链（1.1.1-2/667 阶段）已通过，目标为 helper-first 1.1.1-1：本地物料审计确认 1.1.1-1 与 1.1.1-2 包内 supervisor 完全相同，SHA256 均为 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`，内层成员集合相同，差异仅为 manifest 版本/校验、HTML 版本显示及其压缩物，未发现 schema/layout 变化。前置无未决事务；官方 UI 保留数据卸载 1.1.1-2 返回 HTTP 200/code 0，保留 `@appdata`、`@appconf`、`@apphome`、共享配置和 `supervisor-token` metadata，并移除 package target。随后安装精确旧包，上传 SHA256 为 `2ecbe8bdd8ab7b7710285791a403b33373a9d612040478ec4f1a47452abcf5a8`，App Center 1.1.1-1 为 `running`；Gateway 页面/API `status`/`install/status`/`logs` 均 HTTP 200，`recovery_required=false`，manager 单一，正确的 `app.sock`/control socket 正常，无内核与未决事务。旧版验收首次进程查询为 0，随后 `PROCESS_EXACT_RETRY=1` 得到 `PID_COUNT=1` 且 supervisor SHA256 为预期值，按 retry 结果计。之后仅提交一次官方 `update/task`，`update/status` 终态为 5，最终 1.1.1-2 `running`；最终 FPK SHA256 为 `667193f2053739a6263f0ddc125e5edf6a32ac05f11594c9be6c34ba5b06bf92`，四处 supervisor/helper SHA256 均为预期值，最终 Gateway 与数据/事务状态通过。证据目录：`/tmp/qwenpaw-g3-supported-rollback-20261107/`。该证据不宣称整个 appdata metadata、runtime 文件或 socket 在生命周期中不变，也未读取或校验共享文件内容。
- 此前从 1.1.1-2 直接原位安装精确低版本的提交动作虽 download/task HTTP 200、code 0，但 fnOS 原文为“无法安装 QwenPaw”“已安装相同或更高版本的 QwenPaw”，最终仍为 1.1.1-2 `running`、download code 10030；同版本最终包重试同样被拒绝。该原始响应证明直接原位降级按设计被禁止，不是平台异常、不是本次受支持回退链的失败或替代证据；不得再次尝试直接原位降级。
- 既有 callback trace 摘录 6 行，时间为 `2026-11-06T11:02:17+0800` 与 `2026-11-06T11:38:20+0800`，两组均包含 `upgrade_init.entry/before_exec/after_return` 且 `after_return result=0`，记录 helper/target 存在且可执行；该 trace 使用 `/vol1/appcenter-downloads/qwenpaw-1.1.1-2-tpk/...`，仅为既有平台 trace，不得与当前 667 FPK 身份混称。证据目录为 `/tmp/qwenpaw-g3-sequence-20261107/`，摘要为 `execution-summary-20261107.txt`。
- **历史 1.1.1-2 正式 SIGKILL → 官方恢复子项通过。** 17:38:02.180 的 `fresh_pre_sigkill_readonly_gate` PASS：唯一 manager PID `459807`、starttime `23000247`，binary SHA256 `162b02bbb79eb59b6b084675a121005cdaffb805961392e437fd8c39cd3c49f4`，预期双 UDS 与 pidfiles 门禁通过。17:42:55.090 仅一次 `sigkill_once` exit 0，17:42:55.091 随即仅一次官方 `crash_recovery_start_request` HTTP 200/code 0；恢复身份与 `crash_recovery_success` 均 PASS，新 manager PID `462058`，App Center `running`、Gateway HTTP 200、双 UDS 监听及身份/pidfiles 正常。早期探针和身份不匹配尝试不计入正式子项；本证据不替代完整崩溃/重连矩阵。记录：`/tmp/qwenpaw-g3-lifecycle-20261107/appcenter-state-machine-authenticated-20261107.jsonl`。
- **历史 1.1.1-2 普通系统重启自动接管子项通过。** 普通 `sudo reboot` 前置门禁通过，boot_id 从 `2f296c6f-0e40-4a6d-956d-2d08fa27b96e` 变为 `24485936-0f2d-40a9-8017-cd658dab4dc1`；平台日志 18:05:51 记录 `APP_AUTO_STARTED` 并成功注册 `qwenpaw.console`，无手工 start。18:26:57 全新认证上下文的 GET-only 复核：App Center HTTP 200/code 0、1.1.1-2 `running`；Gateway 页面、status、install-status、install-log、logs 均 HTTP 200；唯一 manager PID `2697`/PPID 1，binary SHA256 与前述一致；两个预期 UDS mode `0600`、UID/GID `994/989` 且由该进程监听，kernel processes 为空、install phase `idle`、`recovery_required=false`。18:29:55 的 `automatic_reboot_gate` PASS，包含 boot_id、App Center、Gateway、manager/Socket 身份、无孤儿内核及无未决恢复事务。仅证明一次正常重启后的自动接管，不证明掉电持久性或完整 App 验收。记录：`/tmp/qwenpaw-g3-lifecycle-20261107/system-reboot-acceptance-20261107.jsonl`。
- **独立 TCP listener 核查为限定范围通过。** `ss -ltnpe` 服务/内核级检查未发现 QwenPaw TCP listener，两个预期 UDS 正常；因当前 SSH 用户不能完成 `/proc/*/fd` inode-to-PID 映射，这不是完整全系统逐 PID FD 审计或全系统监听归属证明。记录：`/tmp/qwenpaw-g3-lifecycle-20261107/lifecycle-blocked.txt`。
- 早期手工 root 恢复及手工 package 启动造成的 App Center 状态不一致和历史 10330/10350，不计为正式恢复失败或当前包缺陷。先前浏览器上下文的 401/轮询超时由全新认证上下文 GET-only 只读复核收口；它既不表示当前 UI/App 验收失败，也不构成 App 验收通过。真实手机 fnOS App 暂不纳入本轮 G3；本地 WebKit/Chromium Gateway 2/2 作为浏览器兼容证据保留。新 UI logout 已导航至 `/login` 并重新登录，但旧 `ost` cookie 仍可访问有效 Gateway page/status/SSE，真实失效会话门禁仍未通过。阿里云真实 `qwenpaw==2.2.2.post1` 上游冒烟已完成，官方 PyPI seq1 因 15 分钟无输出超时失败且未重投。

**历史 G3 退出标准及截至 2026-11-08 的结论：**当时定义为支持版本的桌面/浏览器管理流程、当时迁移路径上的 CGI 与 Gateway 一致性、CGI 回退不重复事务，以及受支持旧包回退链均须满足；手机 App 暂不纳入本轮且不作为阻塞。受支持旧包回退链已按“恢复未决事务并确认 schema/layout 兼容 → 官方保留数据卸载 → 安装精确旧包 → 验证 → 再正式升级”完成。1.1.1-3 已通过 A/B/C/D、remove/重装、回退、生命周期、本地 WebKit/Chromium 浏览器兼容和阿里云真实上游相应子项。匿名/伪造会话的 `200 text/plain invalid token` 不构成授权通过；logout 后旧 `ost` cookie 仍有效，官方 PyPI 直连及部分完整生产路径/错误矩阵未完成。**当前状态（2026-11-09）：**G3 按已批准范围完成、G4 已实施并退役 CGI；未完成的生产发布门禁仍保留，不代表 G3 未完成或 CGI 仍在生产。

### 历史快照：G3 候选期主入口切换与故障恢复（截至 2026-11-08；1.1.1-3 A/B/C/D 已通过，当时整体未退出）

以下复选项和详细结果只描述 2026-11-08 当时状态。**当前状态截至 2026-11-09：**G3 按已批准范围完成，G4 已实施并退役生产 CGI；当前 1.1.1-6 仅为测试候选，不是生产发布。手机 App 最终复测仍待用户且不阻塞当前验收，fnOS logout 后旧会话仍有效是已知平台问题；未完成的生产发布门禁保留为独立事项。

- [x] release candidate 的 `qwenpaw.console` 主入口切换到 Gateway URL/Socket；候选 FPK 保留 CGI bridge 作为迁移期回退物料。
- [x] 前序真实应用验收已通过管理员/普通用户/伪造头/CSRF，以及桌面 Gateway 与 Socket stop/start；1.1.1-3 本轮正式 UI 安装/升级、keep/remove 卸载、helper-first 基线、连续 stop/start/start/reconnect、Socket fail-closed 与恢复和同一 FPK 重装恢复均已通过。
- [x] 最终应用在 QwenPaw 内核未安装时仍可返回 Gateway 状态/日志和诊断；supervisor 停止后应用中心 start 可恢复。
- [x] 1.1.1-2 → 1.1.1-3 正式外壳升级已通过，状态序列和当前桌面入口保持有效；该过程是官方 update 单次升级，不是原位降级。
- [x] 完整受支持回退链已通过：确认无未决事务和旧/新包 schema/layout 兼容，经官方保留数据卸载当前版、安装精确 helper-first 旧包、验证 Gateway/数据/事务与单一 manager 后，仅提交一次官方升级回最终版。直接原位安装低版本被拒绝仅作为按设计禁止原位降级的证据；不得再次尝试直接原位降级。
- [x] 一次正式 SIGKILL 后官方 App Center start/start 恢复通过：精确核验 PID/starttime `459807/23000247` 后仅 SIGKILL 一次、仅提交一次官方恢复，HTTP 200/code 0，恢复至唯一新 manager PID `462058`，身份、双 UDS、pidfiles、App Center 与 Gateway 门禁均通过。
- [x] 一次普通系统重启后的自动接管通过：boot_id `2f296c6f-0e40-4a6d-956d-2d08fa27b96e` → `24485936-0f2d-40a9-8017-cd658dab4dc1`，平台记录 `APP_AUTO_STARTED` 并成功注册 `qwenpaw.console`，无手工 start；全新认证 GET-only 复核与 `automatic_reboot_gate` 均 PASS，唯一 manager PID `2697`、Gateway/双 UDS 正常、无内核进程或未决恢复事务。
- [x] 限定范围 TCP listener 核查通过：`ss -ltnpe` 服务/内核级检查未发现 QwenPaw TCP listener，预期双 UDS 正常；`/proc/*/fd` inode-to-PID 映射不可用，所以不宣称完成全系统逐 PID FD 审计或全系统监听归属证明。
- [x] 1.1.1-3 真实 fnOS C 矩阵已通过：`app.sock` stale、active listener、普通文件、软链接和 unknown owner 均 fail-closed，随后恢复；B 的连续 `stop/start/start/reconnect`、D 的 manager SIGKILL→官方 start 接管及 remove/重装也已通过。证据总目录：`/tmp/qwenpaw-g3-1.1.1-3-20261108/`。本地隔离 stale/active/socket、孤儿内核和 sentinel 矩阵仍仅作为本地补充，不扩大为手机 App 或完整生产发布证据。
- [ ] 真实手机 fnOS App 暂不纳入本轮 G3，不作为阻塞；隔离 Docker WebKit Gateway 2/2 与 Chromium Gateway 2/2 保留为浏览器兼容证据。新 1.1.1-3 Playwright Chromium UI logout 证据（`/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`）显示“我的账号→退出登录”导航至 `/login` 并重新登录成功，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；这是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试，故真实失效会话门禁仍未通过。WebKit 丢回执不是生产 beacon 失败：第三次 beacon `success=false` 复用 seq/instance、不新建事务，测试严格检查 done/upgrading=false/recovery=false、marker/pin/健康并精确匹配跨引擎 console 错误。真实上游冒烟已在阿里云真实 `qwenpaw==2.2.2.post1` 完成，官方 PyPI seq1 因 15 分钟无输出超时失败且未重投。

2026-11-08 本地 Gateway / WebKit / process identity 测试证据：当前最终候选 FPK 为 `build/candidates/1.1.1-3-gateway-g3-20261108/qwenpaw-1.1.1-3-x86.fpk`，SHA256 `94679e4d8343c1bf341e8a1cc2a1451a9ad0ad3296d2fc668deab456db601b51`，supervisor SHA256 `2829f1a4079139f166f78478de804d3f619226d646ea93d58d0965718dbc4400`；该包已通过 build/preflight/audit，并经官方 update 单次升级在远端验收。`supervisor/src/install/process.rs` RED/GREEN 证据 `/tmp/qwenpaw-g3-process-identity-20261107/`，install 87、runtime 5、严格 Clippy、user namespace 全 crate 276+13 通过。WebKit 日志 `/tmp/qwenpaw-gateway-webkit-20261107/e2e-webkit-https.log`；Chromium Gateway 2/2 已由主控复验通过。A/B/C/D 及 remove/重装证据总目录为 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`；真实上游及内核卸载记录分别见 `/tmp/qwenpaw-g3-real-upstream-20261107/` 与 `/tmp/qwenpaw-g3-real-upstream-20261108/kernel-uninstall-final-20261108.json`。

**历史 G3 退出标准/结论（截至 2026-11-08）：**测试设备连续重启、平台 stop/start、包升级、manager SIGKILL 后官方 start、Gateway 重连、当时迁移期 CGI 与 Gateway 行为及授权一致性、受支持旧包回退等作为当时的阶段退出条件。1.1.1-3 已按记录通过 A/B/C/D、remove/重装、既有回退和生命周期子项及本地 WebKit/Chromium 浏览器兼容回归；手机 App 暂不纳入本轮。UI logout 后旧 `ost` cookie 仍有效，未观察到服务端 session revoke；官方 PyPI 直连和部分完整生产路径/错误矩阵仍未完成。**截至 2026-11-09 当前状态：**G3 按批准范围完成，G4 已实施并退役 CGI。尚未完成的生产发布门禁不改变 G3/G4 状态；手机复测仍待用户且不阻塞当前验收。

### G4：正式退役 CGI（已实施）

状态更新至 2026-11-09：G4 已完成，当前生产管理 Web 为 Gateway-only。此次同步按现存工作树记录实际架构；它不表示 1.1.1-6 已作为生产版本发布。

- `app/ui/config` 已切至 Gateway；`qwenpaw.console` 与 `qwenpaw.main` 通过 `/app/qwenpaw/web/` 使用同一 Gateway/Socket。
- `web/src/api.ts` 已移除 api.cgi 地址推导和 action fallback；管理系统 CSRF 与代理至 QwenPaw 的上游请求区分处理。
- 构建 staging、包审计、lint、CI/Release 和审计清单已同步移除 CGI 假设及 CGI 专属检查；Gateway 回归覆盖保留。
- `app/ui/api.cgi`、`app/ui/index.cgi`、生产 `web api-cgi/index-cgi` 子命令和 CGI 专属测试脚本已删除。
- Rust `web_proxy` 适配上游路径、流式 SSE、Bearer Authorization；WebSocket 返回 501，不宣称完整支持。

历史 G4 计划清单用于记录已完成的迁移事项，不再是待办。后续兼容性或手机端复测结果应作为新证据追加，不得恢复 CGI 作为隐式回退。

## 5. 测试和验收方案

### 5.1 Rust 单元/组件测试

- prefix 路径兼容：完整前缀、已剥前缀、恰好双前缀、尾斜线、query、编码 slash、`..`/双重编码/反斜线/NUL。
- 静态资源：白名单、越界软链、隐藏文件、压缩 Accept-Encoding 的 `q=0`、HEAD/404、Content-Type、缓存和 Vary。
- listener 创建：安全 mode/属主、已占用活 Socket、陈旧自有 Socket、非 socket 普通文件、软链接、未知 owner、权限拒绝、清理仅删本实例 inode。
- 身份策略：网关 peer 才可启用身份解析；loopback、private control、普通进程不能靠伪造 Header 获得权限。**历史迁移期用例**也覆盖 CGI 请求不能凭伪造 Header 获权；生产 CGI 已在 G4 退役，CGI 不是当前传输入口。
- 管理员角色矩阵与 CSRF：每个写 API 缺身份/普通用户/伪造 admin/缺 token/错误 token/过期 session均拒绝且零副作用。
- 管理器 readiness 必须同时包含事务恢复完成和 gateway Listener 就绪；kernel 未安装/停止时管理 API 仍可正常响应。

### 5.2 本机集成/黑盒测试

使用临时 `TRIM_*` 根、真实 release supervisor、真实 Unix Listener 和 Rust 假内核；不要把模拟结果写成真机认证证明。

- HTTP over Unix Socket 访问首页、API、CSS/JS（如未 inline）、压缩资源、错误页。
- **历史迁移期对照测试设计：**曾计划比较 CGI compatibility 与 Gateway API 的相同请求体/状态码/事务结果；此 CGI 对照不是当前能力或当前测试目标，CGI 专属测试已随 G4 删除。当前 Gateway 回归验证共用 `ManagementCore` 和并发只提交一次等不变量。
- supervisor 启停、SIGTERM、SIGKILL、路径重建；管理器停止后 service start 可重启；Kernel stop 时 Gateway 仍在。
- 启动配置恢复失败、安装 recovery_required、无内核、坏内核等 degraded 状态都能返回控制台和诊断。
- 安全 peer 单测不能替代真实 fnOS；本机 header 注入用例只验证 Rust拒绝逻辑。
- 最终从 FPK 解出二进制和配置，再跑核心 socket/静态/API黑盒用例，核对测试 binary SHA 与包内 SHA。

### 5.3 fnOS 真机 E2E（发布阻塞项）

至少在 manifest 最低声明版本和项目实际目标版本各做一次完整测试；需要可丢弃设备/VM快照。每次记录 fnOS版本、FPK/SHA、运行用户/Socket权限、脱敏请求/状态码/响应头与系统日志。

1. **未登录**：桌面入口受网关登录保护，不能直接读管理页/状态。
2. **普通用户**：按角色政策拒绝安装、停止、设置、日志/敏感信息；不因可见图标绕过服务器授权。
3. **管理员**：打开首页、刷新、修改镜像/端口/日志级别、start/stop/restart、版本查询/安装任务状态/日志；所有操作走网关。
4. **身份头安全**：客户端附加伪造 `X-Trim-Userid/Isadmin/Username`；确认网关覆盖/剥除。缺失 Header 不得默认为管理员。
5. **CSRF**：HTTPS 和 HTTP（若支持）cookie 属性，跨站表单/请求、无 token、自定义错误 Origin；代理 Host 重写不得拒绝合法请求，也不得放过伪造请求。
6. **网关路径**：iframe、URL、尾斜线、前缀保留/剥离、query、刷新 deep path、404、反向代理错误配置。
7. **真实移动 App（暂不纳入本轮 G3）**：登录态、切后台、session 到期、返回导航、安装任务继续/重连可在后续平台范围恢复时验证；当前不作为 G3 阻塞。本地 WebKit/Chromium 继续作为浏览器兼容证据，不替代移动 App 结论。
8. **流式/长请求**：当前实际用到的 SSE/长轮询在 gateway 下 heartbeat、断连和重连；不支持时按 API fallback 设计并验证。
9. **内核生命周期**：未安装、启动中、安装失败、内核停止、内核正常、recovery_required；management gateway 始终可用且不谎报成功。
10. **管理器生命周期**：appcenter stop/start、系统重启、 supervisor 优雅退出、SIGKILL/强制中断、Socket stale recovery；管理器 down 时网关故障明确，应用中心 start 可恢复。
11. **包升级/受支持回退**：完成 CGI→gateway 候选升级后，先恢复未决事务并确认 schema/layout 兼容，再通过官方保留数据的卸载流程卸载当前版、安装精确旧包，验证旧包版本/入口/数据/运行状态，最后再正式升级最终版。直接原位安装低版本按设计被禁止，不能作为回退演练；至少完成一次真实的受支持回退演练。

### 5.4 最终项目门禁

每阶段按改动范围运行；不在计划阶段预称通过：

```bash
cargo test --locked --manifest-path supervisor/Cargo.toml
cargo clippy --locked --manifest-path supervisor/Cargo.toml --all-targets -- -D warnings
npm test --prefix web
npm run typecheck --prefix web
npm run test:e2e --prefix web
npm run test:e2e:gateway --prefix web
python3 scripts/test-gateway-uds.py
python3 scripts/test-management-boundary.py
python3 scripts/test-platform-callbacks.py
STRICT_CI=1 bash scripts/lint.sh
bash build.sh
bash scripts/audit-fpk.sh build/output/qwenpaw-<version>-x86.fpk
```

G4 已将 CGI 专属测试入口和 CI 步骤替换/删除，Gateway 回归留在当前测试与发布门禁中。构建并测试最终候选 FPK，而非仅源码构建目录；远端 `running` 或源码测试通过均不自动构成生产发布声明。真实手机 fnOS App 最终复测仍待用户。

## 6. 回退与支持版本策略

- **历史开发/首个 Gateway RC 回退策略：**不改变数据 schema；G1 阶段曾保留 CGI 物料与旧主入口，G2 候选再切换主入口。此策略描述历史迁移阶段，不代表当前保留 CGI。若需采用已验证的受支持旧包回退路径，先恢复未决事务并确认 schema/layout 兼容，再通过官方保留数据卸载当前版、安装精确旧 FPK、完成验证，最后正式升级最终版。不得绕过事务恢复，也不得直接向已安装相同或更高版本原位安装低版本。
- **切主入口后**：先停止新写操作，确认 install/settings journal 已恢复；按“官方保留数据卸载当前版 → 安装精确旧包 → 验证 → 再正式升级最终版”执行，升级/回退后的 app entry、Socket、静态资源和数据范围必须作为一组检查。本轮 G3 已按该受支持路径完成真实回退并再次正式升级最终版；直接原位低版本安装仍按设计禁止。
- **旧包不兼容新持久化状态时不回退**。本阶段原则上不增加持久化字段；若确需配置版本字段，先实现旧版可忽略/降级兼容与回滚测试。本轮本地包对比未发现 schema/layout 变化，但这不扩展为共享文件内容已读取或校验。
- **已批准支持下限（2026-11-04；当前契约）：**用户明确批准 `os_min_version=1.2.0800`，manifest 已更新；低于此版本的旧系统不再承诺 CGI 兼容支持，不维护旧系统 CGI 兼容发布路径。**历史迁移期**曾保留 CGI 作为受支持系统的过渡措施；截至 2026-11-09，G4 已实施并退役生产 CGI，不将其作为当前回退手段。受支持旧包回退路径已按记录证据验证，不能以此替代其他生产发布门禁。
- **历史验证与边界（截至 2026-11-08）：**1.1.1-2 → 1.1.1-3 正式 UI/update 升级、前序管理员/普通用户/伪造头/CSRF 与 Gateway/Socket stop/start、A/B/C/D、remove/重装、受支持旧包回退、SIGKILL 后官方恢复、正常系统重启自动接管、限定范围 TCP listener 核查及隔离 Docker WebKit 2/2、Chromium 2/2 均有对应范围的证据；本地 WebKit 不等于真实手机 App。直接原位安装精确低版本收到“已安装相同或更高版本”的拒绝，是平台禁止原位降级的证据，不是有效回退演练，不得重试。手机 App 当时暂不纳入本轮 G3；UI logout 后旧 `ost` 会话仍有效，未观察到服务端 session revoke；匿名/伪造会话仅返回平台 HTTP 200 `text/plain` `invalid token`，不构成授权通过。阿里云真实 `qwenpaw==2.2.2.post1` 上游安装/卸载通过；官方 PyPI 直连 15 分钟无输出超时且未重投，不得宣称官方源通过。回退证据位于 `/tmp/qwenpaw-g3-supported-rollback-20261107/`，历史平台证据位于 `/tmp/qwenpaw-g3-1.1.1-3-20261108/`。**当前状态（2026-11-09）：**G3 按已批准范围完成，G4 已实施并退役 CGI；手机复测仍待用户且不阻塞本轮验收，logout 平台现象未修复。完整生产路径/错误矩阵及其他生产发布门禁仍未全部完成；这不代表 G3 未完成或 G4 尚未开始。
- 真实卸载、删除用户数据、生产升级及部署都不属于本计划自动操作；完整删除用户数据不可回滚，程序/包回退不等于用户数据回退。

## 7. 完成定义

**当前状态（2026-11-09）：**生产管理 Web 已完成 CGI→Gateway-only 迁移，G4 已实施；G0–G3 按已批准范围完成。当前测试候选为 1.1.1-6，远端经官方 update 后为 `running`，不是生产发布。真实手机 fnOS App 最终复测仍待用户；fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收。官方 PyPI 直连 15 分钟无输出超时，阿里云真实上游安装/卸载通过但不等于官方 PyPI 直连通过。

### 历史发布门禁快照（截至 2026-11-08）

以下复选项和状态摘要保留当时验收记录；CGI 退役及 1.1.1-6 候选状态请以本文顶部的 2026-11-09 当前摘要为准。

- [x] 已批准最低/目标版本 `1.2.0800` 的完整应用 Gateway 路径、用户身份/权限及 Gateway Socket stop/start 有真实平台证据；本轮同一未刷新面板的 stop/start 自动恢复也通过。
- [x] 已实测完整应用管理页面/API 经 Gateway 可达（包括内核未安装时 status/logs 仍正常响应）。
- [x] 服务/内核级 TCP listener 核查未发现 QwenPaw TCP listener，预期 Gateway/control UDS 正常；这是受限范围证据，不是完整全系统逐 PID FD 审计或监听归属证明。原始记录：`/tmp/qwenpaw-g3-lifecycle-20261107/lifecycle-blocked.txt`。
- [x] 完整应用管理员、普通用户、客户端伪造 `X-Trim-*` 头及 CSRF 矩阵已有前序真实验收。
- [ ] 1.1.1-3 UI logout 已通过“我的账号→退出登录”导航至 `/login` 并重新登录，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；证据为 `/tmp/qwenpaw-g3-1.1.1-3-20261108/matrix-d-web-session-logout-playwright-20261108.json`。这是当前 fnOS 公开 logout 未观察到服务端 session revoke 的平台限制，不是应用层鉴权绕过，也不是自然时间过期测试；真实失效会话仍需按完整平台 E2E 补齐。匿名/伪造会话当前仅得到平台 HTTP 200 `text/plain` `invalid token`，不得把该响应代替为登录/管理员授权通过。
- [x] QwenPaw 内核未安装时管理页面和 API 仍可用；supervisor 停止后应用中心 start 可恢复，stop/start 的失败与恢复边界已有真实证据。
- [x] 一次精确身份 SIGKILL 后官方 App Center start/start 恢复通过（PID/starttime `459807/23000247` → 新 manager PID `462058`）；一次普通系统重启后的自动接管通过（boot_id `2f296c6f-0e40-4a6d-956d-2d08fa27b96e` → `24485936-0f2d-40a9-8017-cd658dab4dc1`，最终 manager PID `2697`）。这两项只关闭相应单次场景，不代表完整生命周期矩阵。
- [x] 1.1.1-3 C 已覆盖独立陈旧 `app.sock`、active listener、普通文件、软链接和 unknown owner 的 fail-closed 与恢复；B 已覆盖连续 `stop/start/start/reconnect`，D 已覆盖 manager 接管、sentinel、remove 与重装。完整生产故障/错误矩阵仍需另行完成，真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；正常重启与单次 SIGKILL 证据不能替代失效会话和其他剩余门禁。
- [ ] 真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；隔离 Docker WebKit Gateway 2/2 与 Chromium Gateway 2/2 继续作为浏览器兼容证据。新 1.1.1-3 UI logout 已导航至 `/login` 并重新登录，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE；真实失效会话仍未通过。阿里云真实 `qwenpaw==2.2.2.post1` 上游下载、运行与最终内核卸载冒烟已完成；官方 PyPI seq1 因 15 分钟无输出超时失败且未重投。匿名/伪造页面/API/SSE 仍仅为 `200 text/plain invalid token`，不构成授权通过。
- [x] 1.1.1-2 → 1.1.1-3 正式 UI/update 升级已通过；keep/remove 卸载范围、同一 FPK 重装恢复及最终 `1.1.1-3 running` 状态均按当前证据记录。
- [x] 历史完整受支持旧包回退链（1.1.1-2/667 阶段）已通过：先确认无未决事务及 schema/layout 兼容，再经官方保留数据卸载流程卸载当前版、安装并验证精确旧包，最后仅提交一次官方升级并验证最终版。数据范围仅按已取得的路径和 metadata 证据描述；不得据此宣称整个 appdata metadata 不变或共享文件内容已校验。直接原位降级被拒绝仅证明按设计禁止，不是有效回退演练；不得再次尝试直接原位降级。原始记录：`/tmp/qwenpaw-g3-supported-rollback-20261107/`。
- [ ] 最终 FPK 不含 CGI 的条件未满足：CGI 仍是候选回退物料；CGI 退役、CI/审计清理和全部支持版本验证尚未开始。

**历史状态快照（截至 2026-11-08）：最低 `os_min_version=1.2.0800` 已批准并写入 manifest；G0/G1 已完成，G2 本地实现、Chromium 回归和候选包内复验已完成；当时 1.1.1-3 已从 1.1.1-2 经官方 update 单次升级并完成远端 A/B/C/D 验收：完整请求矩阵、连续 stop/start/start/reconnect、真实 app.sock stale/active/占位文件/软链接/unknown owner fail-closed 与恢复、阿里云真实上游、manager SIGKILL→官方 start 接管、remove 范围和同一 FPK 重装均通过。最终 App Center 为 `1.1.1-3 running`，唯一 manager、Gateway HTTP 200、无 QwenPaw 内核/未决事务/18088 listener，双 UDS 为 `0600`、UID/GID `994/989`。匿名/伪造会话仅返回平台 HTTP 200 `text/plain` `invalid token`，不构成授权通过；真实手机 fnOS App 暂不纳入本轮 G3、不作为阻塞；UI logout 已导航至 `/login` 并重新登录，但未观察到 logout/logoff 等非 GET 请求，`ost` 仍保留，旧 cookie 的 Gateway page/status/SSE 仍为 HTTP 200 的有效 HTML/JSON/SSE。官方 PyPI 直连、完整生产路径/错误矩阵和发布门禁仍未完成；当时记录 G3 未整体退出、G4 尚未开始且 CGI 仍保留。历史直接原位降级被平台按设计拒绝，不是回退失败或平台异常，不得再次尝试。本快照不覆盖当前状态：截至 2026-11-09，G3 已按已批准范围完成，G4 已实施并退役生产 CGI；当前测试候选 1.1.1-6 及远端状态见本文顶部，仍不代表生产发布。本次仅更新文档，未执行平台操作。**
