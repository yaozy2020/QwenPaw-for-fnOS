# QwenPaw FPK — 审计报告

> ⚠️ 本文件由 `scripts/gen-audit.py` 自动生成，请勿手工编辑。
> 数据源：manifest / cmd / wizard / app / supervisor Gateway testdata / .github / git tags。

## 当前版本：v1.3.3-2

- **生成时间**：2026-11-18 23:09:11 +0800
- **应用名**：qwenpaw
- **显示名**：QwenPaw
- **依赖**：nodejs_v24:python312
- **服务端口**：?
- **fnOS 最低版本**：1.2.0800

## 工程统计

| 项目 | 数据 |
|:-----|:-----|
| 生命周期脚本 | 9 个 |
| 向导清单 | 4 个 |
| 上游 Python 文件 | 0 个，0 行 |
| 上游 Plugins | 0 个 .py |
| CI Workflow | 2 个 |

## 生命周期脚本清单

| 脚本 | 说明 |
|:-----|:-----|
| `cmd/config_callback` | 配置变更后回调（fnOS 控制台修改配置时触发） |
| `cmd/config_init` | 配置初始化 |
| `cmd/install_callback` | 安装完成回调 |
| `cmd/install_init` | 安装前环境检查 |
| `cmd/main` | 应用启动 / stop / status 入口 |
| `cmd/uninstall_callback` | 卸载完成回调 |
| `cmd/uninstall_init` | 卸载前清理 |
| `cmd/upgrade_callback` | 升级完成回调 |
| `cmd/upgrade_init` | 升级前环境检查 |

## 向导清单

| 文件 | 阶段 |
|:-----|:-----|
| `wizard/config` | config |
| `wizard/install` | install |
| `wizard/uninstall` | uninstall |
| `wizard/upgrade` | upgrade |

## QwenPaw base-path 与预构建 Console 补丁载荷

### QwenPaw 2.3.0.post1

- 上游 PR：`#8719`（临时下游兼容，不表示上游已合并）
- 后端及预构建 Console 差异文件：1135 个，原始数据 54,297,348 字节
- manifest SHA256：`cde9b88a6aa95090eff16ce33808dad4270c414c32447b85ac7fdd77310c920e`

- FPK 携带后端补丁和完整预构建 Console；安装时不再下载源码或运行 npm/Vite

## Gateway 源码回归夹具

> 以下文件是源码测试夹具，不属于 FPK 物料清单。

| 文件 | SHA256 | 用途 |
|:-----|:-------|:-----|
| `supervisor/src/gateway/testdata/qwenpaw-2.2.2.post1-chat.css` | `ffdc67228cded582e85575fd3e6d69f6b4fb381392e540a39c16646384e0fc87` | Gateway 上游路径/协议回归夹具 |
| `supervisor/src/gateway/testdata/qwenpaw-2.2.2.post1-entry.js` | `e35db2c040031038762fc91d0f08b1747d1c346e20140c1b5ae13b6a90da9354` | Gateway 上游路径/协议回归夹具 |
| `supervisor/src/gateway/testdata/qwenpaw-2.2.2.post1-index.html` | `7c2f6cc4bdc8bbd242d41c998bd841884b26a12a5c4eb5d4dc8feffdb80400ef` | Gateway 上游路径/协议回归夹具 |
| `supervisor/src/gateway/testdata/qwenpaw-2.2.2.post1-login.js` | `b2b0449357aaec831093bf65ffc78d1e10f92ed2b5faeb9ab37cf39f0e2e3969` | Gateway 上游路径/协议回归夹具 |
| `supervisor/src/gateway/testdata/qwenpaw-2.3.0-chat.js` | `b37737563f7518211bd1f06ffd1bf5a54d9d05c5f536a84af42ee9991a8f2473` | Gateway 上游路径/协议回归夹具 |
| `supervisor/src/gateway/testdata/qwenpaw-2.3.0-login-logo.js` | `0e76446ec7641a8543a7b3bb582abc000cb149038b0d8627405746551de9bb47` | QwenPaw 2.3.0 登录 Logo 路径回归；不代表完整 base-path 契约 |

## CI/CD

| Workflow | 触发 |
|:---------|:-----|
| `ci.yml` | PR 触发，跑 preflight + 前端构建验证 |
| `release.yml` | tag 触发，fnpackup 容器构建并自动上传 Release |

## 最近发布记录

| 版本 | 发布日期 |
|:-----|:----:|
| `v2.1.0-rc.3` | 2026-11-03 |
| `v2.1.0-rc.2` | 2026-11-03 |
| `v2.1.0-rc.1` | 2026-11-02 |
| `v2.0.1` | 2026-11-02 |
| `v2.0.0-rc.1` | 2026-11-02 |

---

生成命令：`python3 scripts/gen-audit.py`
