# Playwright 本地管理流程 E2E

Gateway E2E 的真实集成流程由浏览器经本地 HTTP(S) 测试适配器进入 release `qwenpaw-supervisor` 的 UDS Gateway，Rust Gateway 与管理逻辑均为真实实现。测试使用工程测试用 `qwenpaw-test-runtime`，因此不等于真实 QwenPaw 内核或公网依赖验收。Gateway 的合成测试身份不构成 fnOS 身份或会话验收。

`Web proxy port UI handles validation, conflict and successful automatic selection` 是一个有意隔离的 UI 测试：Playwright 只 mock `/api/web-proxy/port`，检查前端校验、错误呈现、自动端口请求及 CSRF header。该 API 请求不经过真实 Gateway 或 manager，不作为后端行为证据；页面和其他未拦截请求仍经 Gateway。真实端口 API、冲突保留和 listener 切换由下方 opt-in 的 `real Gateway Web proxy port UI...` 测试验证。

## 运行

在仓库根目录：

```bash
npm ci --prefix web
npm run test:e2e --prefix web                     # Gateway UDS，默认 Chromium
npm run test:e2e:gateway --prefix web             # 同一 Gateway 流程
npm run test:e2e:gateway:webkit --prefix web      # 同一 Gateway 流程，WebKit（本地 HTTPS）
```

默认夹具关闭独立 Web 反代监听器，因此真实端口切换用例会跳过。需单独启用自动分配并运行该真实 Gateway 用例：

```bash
QWENPAW_E2E_WEB_PROXY_PORT=0 npm run test:e2e:gateway --prefix web -- --grep 'real Gateway Web proxy port UI'
```

该用例不 mock 端口 API：经真实 Gateway 读取端口，验证占用冲突返回 409 且保留原端口，提交 `0` 后检查页面、持久化端口和新旧监听器。测试内核未运行时，切换前旧监听器与切换后新监听器返回的 HTTP 503 只用于确认反代监听器可达；随后还会检查旧监听器已关闭。这不代表真实内核页面已验证。

也可以在 E2E 命令后显式选择引擎，或经环境变量选择（默认仍为 Chromium）：

```bash
npm run test:e2e:gateway --prefix web -- --engine=webkit
QWENPAW_BROWSER_ENGINE=webkit npm run test:e2e --prefix web
```

WebKit Gateway E2E 使用仅限测试的本地 HTTPS：fixture 在临时目录中通过 OpenSSL 生成带 `127.0.0.1` SAN 的短期自签名证书，退出时随临时目录清理；仅该测试的浏览器和 readiness 检查忽略此证书的信任错误，以便验证 Secure cookie 的真实浏览器行为。Chromium Gateway 默认仍使用 HTTP。该配置不代表生产证书信任、fnOS 真机或 App 容器验收。

Runner 会先用现有 `build.mjs` 在临时目录构建前端，再执行 `cargo build --locked --release --manifest-path ../supervisor/Cargo.toml`，最后运行所选引擎。临时构建不改动 `web/dist` 或 `app/www`。WebKit/Playwright 浏览器可按锁定的 `@playwright/test` 版本安装到当前用户缓存：

```bash
(cd web && npx playwright install webkit)
```

本机不使用 `--with-deps`；缺少系统库时，Playwright 会报出启动错误，应记录为环境阻塞而不是改用 DOM 模拟。开发机若使用系统 Chromium，可显式指定：

```bash
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/usr/bin/chromium npm run test:e2e:gateway --prefix web
```

CI 若需运行对应引擎，应先用该锁定版本安装所需浏览器及 runner 依赖，再执行对应 E2E 命令；缺少浏览器、release 二进制或测试 gateway 就绪必须失败，不得静默跳过。真实端口切换用例只有设置 `QWENPAW_E2E_WEB_PROXY_PORT=0` 才会运行，默认跳过不构成通过。`PLAYWRIGHT_CHROMIUM_EXECUTABLE` 显式设置但不存在时也会直接失败。

## 验收边界

- 每次运行使用 `/tmp/qwenpaw-browser-gateway-*` 临时根目录、临时端口和独立 supervisor token；退出时核验 supervisor 进程身份、回收受管后代、确认 supervisor/test-runtime 端口不可达并清理临时目录。
- 测试 HTML 副本中的四个生产 PyPI 镜像 URL 会在临时目录内改写为本地 `/mirror/*` URL，由独立临时 loopback HTTP 端口提供；WebKit 的 HTTPS 浏览器入口与该 HTTP 镜像端口分离。这样版本列表和安装仍由真实 Gateway/supervisor 的 HTTP/HTML 解析、事务和 test-runtime 健康检查驱动，但不宣称已验证公网 PyPI、真实 fnOS 网络或上游下载。
- 安装包本身由工程测试 runtime 提供，不是 QwenPaw 生产 Python 包；它只用于受管运行时契约和 `/api/version` 健康检查。该边界不能冒称真实上游内核验收。
- 测试包含一次故意丢弃 upgrade 受理响应的场景，要求界面显示“无法确认安装任务结果”，同时通过真实 `seq`/`instance`、后端状态和日志确认不会重复受理任务。
- 另一个恢复测试通过 Playwright 临时阻塞一条 install-log 请求，并模拟 `sessionStorage` 不可用；它验证前端恢复与轮询行为，不改变其他请求的 Gateway 或 Rust 管理实现。
- Gateway E2E 中的旧 ingress 请求只作为 404 负向断言，不会执行或调用旧入口。
- 该流程未运行 fnOS 真机 WebKit/App 容器、真实上游网络、FPK 安装/升级或生产用户数据操作。

## 依赖许可证

Playwright 依赖固定为 `1.56.1`，只加入 `web` 的开发依赖，不进入生产包：`@playwright/test`、`playwright` 和 `playwright-core` 使用 Apache-2.0；其可选 Darwin `fsevents` 使用 MIT。现有 `esbuild`/`preact` 为 MIT，TypeScript 为 Apache-2.0。完整版本、完整性摘要和许可证字段以 `web/package-lock.json` 为准。
