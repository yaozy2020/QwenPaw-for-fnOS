# QwenPaw for fnOS 1.4.3-1 实机浏览器验收交接

> 用途：交给本机测试会话，完成当前候选的最后两项浏览器验收。
> 本文只要求验证，不要求改代码、重装、改密码或发布。

## 1. 被测对象

- 项目：`QwenPaw-for-fnOS`
- 候选版本：`1.4.3-1`
- manifest：`manifest`
- 已安装路径：`/var/apps/qwenpaw`
- QwenPaw 服务端口：`18088`
- 独立反代端口记录：`19091`
- 运行时接口版本：`2.3.0.post1`
- 管理页面前缀：`/app/qwenpaw/web/`
- 本地工作区：`/vol2/1000/下载/QwenPaw-for-fnOS`

本候选适配上游 PR #8719 的运行时 base-path 修复，重点覆盖：

1. 直开 `index.html` 时资源和 `<base>` 不丢失；
2. 中文或包含空格的前缀登录跳转不脱离前缀；
3. 语音回调和 WebSocket 的内部转发不把外部前缀错误传给 worker；
4. 原生插件在带前缀部署下仍能工作。

## 2. 已完成的验证，不要重复解释为待验收

以下证据已经在当前本机实例完成：

- 应用 manifest 为 `1.4.3-1`。
- 已安装包的 1136 个文件摘要全部匹配包内 manifest。
- `GET /` 返回 `307`，跳转到 `/app/qwenpaw/web/`。
- `/app/qwenpaw/web/`、`/app/qwenpaw/web/index.html`、`/app/qwenpaw/web/login` 均正常返回。
- 页面 `<base>` 为 `/app/qwenpaw/web/`。
- JavaScript、CSS 和入口资源均返回 `200`，与已安装补丁摘要一致。
- 本机 Chromium 无界面加载实际服务时，没有失败请求和控制台 error/warning；页面正确到达登录页。
- `/api/auth/status` 显示鉴权已启用且已有用户。

可复跑的只读检查：

```bash
cd /vol2/1000/下载/QwenPaw-for-fnOS
python3 .pr8719-adaptation-work/check-live.py
```

预期关键输出：

```text
Installed package matches all 1136 file hashes
native / 200 -> /app/qwenpaw/web/
Served entry assets match the installed patch
/app/qwenpaw/web/api/version {'version': '2.3.0.post1'}
```

## 3. 本轮必须完成的验收

### A. 飞牛桌面原生入口和登录到聊天页

使用本机真实浏览器，先登录飞牛 fnOS 管理桌面，再从桌面应用图标打开 QwenPaw。不要只访问 `127.0.0.1:18088`，因为那会绕过飞牛统一网关，不能证明桌面内嵌入口正常。

记录以下证据：

1. 桌面点击后实际打开的页面或 iframe URL；
2. 页面是否始终保留 `/app/qwenpaw/web/` 前缀；
3. 浏览器网络面板中是否出现根路径资源请求，例如：
   - `/assets/...`
   - `/api/...`
   - `/login`（不含前缀）
4. 登录前页面是否能正常显示；
5. 使用原生账号登录后：
   - 地址仍在 `/app/qwenpaw/web/` 下；
   - 不再停留在 login 页面；
   - 聊天输入框或编辑区域可见；
   - 页面刷新后仍能恢复聊天页；
   - 没有资源/API 404。

正常结果应类似：

```text
入口：.../app/qwenpaw/web/
登录：.../app/qwenpaw/web/login
登录后：.../app/qwenpaw/web/chat
JS/CSS/API：全部位于对应前缀下，无 404
聊天输入框：可见
刷新聊天页：成功
```

如果飞牛桌面本身直接建立原生会话、未出现 QwenPaw 登录页，也视为正常，但必须记录登录后的聊天页和网络请求证据。

### B. QwenPaw Creator（仅在本机已安装时测试）

不要因为插件未安装而修改安装状态。若 Creator 已安装：

1. 打开 Creator 的模型设置；
2. 确认模型列表正常出现，不显示“没有供应商”之类的误报；
3. 在网络面板确认模型请求使用当前部署前缀，不能写死根路径 `/api/models`；
4. 请求成功状态为 `200`，响应不是空列表；
5. 页面控制台无因前缀产生的 `404`。

重点回归此前的问题：带 `/app` 部署时，正确接口可用，但旧组件曾错误请求根路径 `/api/models`，将 404 误显示为没有供应商。

### C. QwenPaw-Data（仅在本机已安装时测试）

不要因为插件未安装而修改安装状态。若 Data 已安装：

1. 打开分析控制台入口；
2. 确认插件入口 HTTP 状态为 `200`；
3. 确认分析控制台 iframe 实际创建并可见；
4. 页面不显示 `Analysis console installation is incomplete`；
5. iframe 的静态入口和 API 请求都保留当前前缀；
6. 控制台没有探测根路径 `/api/frontend_plugin/...` 导致的 404。

重点回归此前的问题：旧组件曾固定探测根路径，带前缀时误报安装不完整且不创建 iframe。

### D. 语音和 WebSocket（如果测试环境具备可触发条件）

这是可执行的额外回归，不要因没有真实语音设备而伪造通过：

- 外部请求带 `/app` 前缀时，worker 内部收到的目标应是 `/voice/...` 或对应无前缀内部路径；
- 不应把 `/app/voice/...` 再转发给 worker；
- 语音回调不能因前缀返回 404；
- WebSocket 握手不能因前缀返回 404；
- 外部签名校验仍使用包含前缀的原始 URL。

如果没有可用语音/WS 测试条件，报告为“未执行”，不要报告为通过。

## 4. 推荐测试方式

### 真实桌面验收

优先使用本机真实浏览器：

1. 登录 fnOS 管理页面；
2. 打开 QwenPaw 桌面入口；
3. 开发者工具勾选 Preserve log；
4. 依次记录页面 URL、iframe URL、登录前后路径、网络失败请求和控制台错误；
5. 完成 A，再按实际安装情况完成 B/C。

### 直接服务只读复核

用于确认不是网关显示问题：

```text
http://127.0.0.1:18088/
```

该入口预期会重定向到：

```text
/app/qwenpaw/web/
```

直接服务入口不能替代飞牛桌面原生入口验收。

## 5. 禁止事项

- 不卸载、不重装、不升级、不降级 FPK。
- 不重置、删除或更换 QwenPaw 账号密码。
- 不修改端口、反代、鉴权、飞牛网关或 SSH 配置。
- 不删除用户数据、插件数据、工作区或备份。
- 不用 `git reset`、`git checkout`、`git clean` 覆盖当前工作树。
- 不手工编辑 `web/dist/`、`app/www/`、`app/bin/`、`supervisor/target/` 等生成目录。
- 不把测试密码、Cookie、Authorization、完整登录响应或用户数据写入报告。
- 不把隔离测试、无界面 Chromium 测试写成飞牛桌面原生验收通过。
- 不把“接口返回 200”单独当作插件功能通过，必须确认页面行为和前缀资源。

当前通过 SSH 建立 WebBridge 隧道曾返回 `administratively prohibited`，说明服务端禁止 TCP forwarding；不要反复尝试或绕过 SSH 策略。`code-rc.kimi.com` 登录授权也不等于浏览器扩展已经连接。

## 6. 结果回传格式

请按以下格式回传，失败项附 URL、状态码、控制台错误或截图路径；截图不得包含密码、Cookie 或 token：

```text
被测版本：1.4.3-1
被测设备/入口：
浏览器及版本：

A 飞牛桌面原生入口：PASS / FAIL / BLOCKED
- 打开入口：
- 登录前路径：
- 登录后路径：
- 聊天输入框：
- 刷新后：
- 404/控制台错误：

B Creator：PASS / FAIL / NOT_INSTALLED / BLOCKED
- 模型列表：
- 请求前缀：
- 404/错误：

C Data：PASS / FAIL / NOT_INSTALLED / BLOCKED
- 插件入口：
- iframe：
- 请求前缀：
- 404/错误：

D 语音/WebSocket：PASS / FAIL / NOT_EXECUTED
- 观察到的外部路径：
- worker 内部路径：
- 状态码/握手：

结论：
未验证项：
证据位置：
```

## 7. 通过标准

只有同时满足以下条件，才能报告“本轮浏览器验收通过”：

- A 为 `PASS`；
- 如果 Creator 已安装，B 必须为 `PASS`；
- 如果 Data 已安装，C 必须为 `PASS`；
- 未出现前缀丢失、根路径资源 404、登录成功后回到根路径、插件误报安装不完整等问题；
- 所有未执行项目明确标注，不能用其他测试替代。

本文件只描述验收交接，不代表在没有实机浏览器证据前已经完成发布放行。
