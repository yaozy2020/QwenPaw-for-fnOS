# QwenPaw for fnOS

[![Version](https://img.shields.io/badge/version-1.1.2-15-blue)](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-green)](LICENSE)
[![fnOS](https://img.shields.io/badge/fnOS-%E2%89%A5%201.2.0800-orange)](https://www.fnnas.com/)

将 [QwenPaw](https://github.com/agentscope-ai/QwenPaw) 安装到飞牛 NAS 的外壳应用。外壳负责安装、运行、配置和运维控制台；QwenPaw 本体由用户从 PyPI 镜像选择版本并安装，不打包进 FPK。

这是由 yaozy2020 维护的第三方打包项目，与 QwenPaw 上游团队无隶属关系或官方背书。

## 功能

- 安装、停止、启动和重启 QwenPaw 内核。
- 查看运行状态、端口、日志和安装进度。
- 选择稳定版、测试版或历史版本；安装失败时保留或恢复原运行环境。
- 修改下载源、内核端口、独立 Web 反代端口和日志级别。
- 手机或浏览器切换页面后恢复正在进行的安装任务。
- 仅卸载内核并保留配置、记忆和管理员认证，或按卸载向导彻底删除应用数据。

## 安装

### 环境要求

- fnOS **1.2.0800 或更高版本**。
- `nodejs_v24`、`python312` 依赖由 fnOS 按应用声明处理。
- 使用 Ollama 时，需另行安装并配置 Ollama 应用。

### 安装外壳

1. 从 [Releases](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases) 下载 `.fpk`。
2. 在飞牛应用中心选择「手动安装」并上传文件；手机文件选择受限时，可先放入 NAS 目录，再选择「从 NAS 添加」。
3. 按向导设置下载源、内核端口和日志级别。
4. 安装完成后打开「QwenPaw 控制台」，选择内核版本并开始安装。

内核 wheel 及依赖需要联网下载，首次安装耗时取决于镜像速度、磁盘和依赖数量。安装使用 uv 并发解析依赖、读取元数据并复用持久缓存，避免国内镜像逐个下载完整候选包后才能解析的问题。国内网络可优先使用阿里云、清华等可用镜像；不要在外壳升级过程中重复提交内核安装。

## 访问方式

| 入口 | 地址/位置 | 用途 |
|:--|:--|:--|
| QwenPaw 控制台 | fnOS Gateway `/app/qwenpaw/` | 安装、启停、日志和设置 |
| QwenPaw 主 Web | fnOS Gateway `/app/qwenpaw/web/` | 对话及上游 Console |
| 独立 Web 反代 | 控制台显示的动态 TCP 端口 | 不经过 fnOS Gateway 的主 Web 访问 |

独立图标和控制台按钮均通过 fnOS Gateway 打开主 Web，不直接跳转内核端口。内核仅监听 `127.0.0.1`；独立 Web 反代负责外部 TCP 访问，不挂载外壳管理 API，也不接受伪造的 fnOS 身份头。

反代端口支持自动分配和手动修改。启动时若目标端口已被占用，会自动选择空闲端口；修改时先绑定新端口，成功后再切换，失败保留原端口。当前实际端口以控制台显示为准。

上游 WebSocket 目前不由此外壳代理，相关请求返回 501；主 Console 的 HTTP、鉴权和 SSE 流程走 Gateway 或独立反代。独立反代默认仅监听回环地址；开启外部访问时，受保护 API 仍需有效的 QwenPaw 登录会话。首次进入上游鉴权页默认使用中文，已有浏览器语言选择不会被覆盖。

## 端口与下载源

- **内核端口**：供本机反代和健康检查使用，默认 `18088`，可在控制台修改。
- **独立 Web 反代端口**：与内核端口分开配置，输入 `0` 自动分配。
- **下载源**：版本列表和 pip 安装使用同一镜像；安装失败后先检查网络，再更换镜像重试。

## 升级与回退

外壳升级不改变用户主动选择的内核版本。已有运行环境正常时不重新安装内核；环境缺损且存在精确版本钉时，才按该版本修复。

fnOS 不允许直接原位安装低版本 FPK。需要回到旧外壳时，先确认没有未决事务及数据格式不兼容，再通过官方「保留数据卸载 → 安装旧包 → 验证」流程操作。不要直接覆盖安装低版本。

早期 2.x RC 属于同一项目的旧编号；从 1.1.2 起固定使用 1.x 序列。曾安装 2.x RC 的测试设备需先保留数据卸载，再安装当前版本，不能原位安装数值更低的包。

## 卸载与数据

- **保留数据**：保留配置、记忆和认证信息，删除当前 venv；重新安装时需按版本钉恢复运行环境。外壳更新流程会保留运行环境，非破坏性更新无需重建。
- **彻底删除**：删除应用运行环境及数据目录，不可通过重装恢复。

仅卸载内核不等于删除用户数据。完整删除前请自行确认不再需要相关数据。

## 常见问题

### 控制台提示内核未安装

这是首次安装外壳后的正常状态。进入「版本更新」，选择版本并安装；安装完成后内核自动启动。

### 安装失败或提示找不到版本

检查当前镜像是否可访问、是否包含所选版本。镜像返回 403 或网络超时时，切换其他可用镜像后重试。不要同时提交多个安装任务。

### 页面提示 CSRF 或来源校验失败

强制刷新控制台以重新同步页面凭据；仍失败时检查是否使用了正确的 fnOS Gateway 地址，不要改用未经支持的代理路径。

### 手机上传 FPK 失败

优先使用电脑浏览器，或将文件放入 NAS 后从 NAS 本地目录选择。

### 使用 Ollama 时 API Key 怎么填

填写任意非空字符串（例如 `ollama`），不要把真实其他服务的密钥复用到此处。

## 开发

生产管理层使用 Rust，前端使用 TypeScript/Preact。fnOS `cmd/*` 仅转发固定生命周期子命令；QwenPaw 本体不在本仓库中。

```bash
cargo test --locked --manifest-path supervisor/Cargo.toml
cargo clippy --locked --manifest-path supervisor/Cargo.toml --all-targets -- -D warnings
npm test --prefix web
npm run typecheck --prefix web
npm run test:e2e:gateway --prefix web
bash scripts/preflight.sh
STRICT_CI=1 bash scripts/lint.sh
bash build.sh
```

完整测试使用隔离临时目录和测试 runtime，不操作已部署服务或用户数据。`build.sh` 负责 musl 静态 release 构建、控制台打包、FPK 生成和内容审计；构建主机需安装 `musl-tools`，Rust 版本与 target 由 `rust-toolchain.toml` 固定。

## 反馈与许可证

- [提交问题](https://github.com/yaozy2020/QwenPaw-for-fnOS/issues)
- [QwenPaw 本体](https://github.com/agentscope-ai/QwenPaw)
- [飞牛 NAS](https://www.fnnas.com/)

本项目及 QwenPaw 原项目均采用 [Apache-2.0](LICENSE) 许可证。
