# Chromium 真实管理流程验收

这组 Playwright 测试使用真实 Chromium，通过临时 CGI gateway 调用构建出的 native `index.cgi`/`api.cgi`、release `qwenpaw-supervisor` 和工程测试用 `qwenpaw-test-runtime`。测试不 mock 浏览器的 `fetch`，也不替换生产 CGI 的业务逻辑；网关只负责把真实 CGI 接到本机 HTTP、隔离 `TRIM_*` 路径，并记录可审计的受理序号。

## 运行

在仓库根目录：

```bash
npm ci --prefix web
npm run test:e2e --prefix web
```

`test:e2e` 会先构建前端，再执行 `cargo build --locked --release --manifest-path ../supervisor/Cargo.toml`，最后启动 Chromium 测试。开发机若使用系统 Chromium，可显式指定：

```bash
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/usr/bin/chromium npm run test:e2e --prefix web
```

CI/Release 应在上传或打包前、release supervisor 构建完成后执行：

```bash
(cd web && npx playwright install --with-deps chromium)
CI=1 npm run test:e2e --prefix web
```

本子任务不修改 `.github/workflows`；workflow 接入应保留上述顺序，并让缺少 Chromium、release 二进制、CGI 或测试网关就绪失败，不得静默跳过。`PLAYWRIGHT_CHROMIUM_EXECUTABLE` 若显式设置但不存在也会直接失败。

## 验收边界

- 每次运行使用 `/tmp/qwenpaw-browser-cgi-*` 临时根目录、临时端口和独立 supervisor token；退出时核验 supervisor 进程身份、回收受管后代、确认 supervisor/test-runtime 端口不可达并清理临时目录。
- 测试 HTML 副本中的四个生产 PyPI 镜像 URL 会在临时目录内改写为本地 `/mirror/*` URL。这样版本列表和安装仍由 native supervisor 的真实 HTTP/HTML 解析、事务和 test-runtime 健康检查驱动，但不宣称已验证公网 PyPI、真实 fnOS 网络或上游下载。
- 安装包本身由工程测试 runtime 提供，不是 QwenPaw 生产 Python 包；它只用于受管运行时契约和 `/api/version` 健康检查。该边界不能冒称真实上游内核验收。
- 测试包含一次故意丢弃 upgrade 受理响应的场景，要求界面显示“无法确认安装任务结果”，同时通过真实 `seq`/`instance`、后端状态和日志确认不会重复受理任务。
- 该流程未运行 fnOS 真机 WebKit/App 容器、真实上游网络、FPK 安装/升级或生产用户数据操作。

## 依赖许可证

Playwright 依赖固定为 `1.56.1`，只加入 `web` 的开发依赖，不进入生产包：`@playwright/test`、`playwright` 和 `playwright-core` 使用 Apache-2.0；其可选 Darwin `fsevents` 使用 MIT。现有 `esbuild`/`preact` 为 MIT，TypeScript 为 Apache-2.0。完整版本、完整性摘要和许可证字段以 `web/package-lock.json` 为准。
