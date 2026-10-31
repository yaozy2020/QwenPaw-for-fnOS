# QwenPaw for fnOS

[![Version](https://img.shields.io/badge/version-1.0.4-blue)](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-green)](LICENSE)
[![fnOS](https://img.shields.io/badge/fnOS-%E2%89%A5%201.0.0-orange)](https://www.fnnas.com/)

将 [QwenPaw](https://github.com/agentscope-ai/QwenPaw) 一键部署到飞牛 NAS 的**轻量外壳包（Launcher）**。

本项目不是 QwenPaw 本体，也不打包本体源码：外壳只负责部署与运维，内核按需从 PyPI 在线安装，二者**版本双轨、互不牵连**。

## 外壳特性

- **极简运维控制台** — 单文件零构建（无前端依赖），白天/暗夜双主题：服务启停与重启、运行状态、日志查看与清理、配置备份下载
- **内核版本管理** — 控制台内自选版本升级或降级（带降级风险提示），内核经 PyPI 在线安装
- **版本双轨制** — 外壳（fpk）升级**不改变**已安装的内核版本；内核版本由"版本钉"记录，运行环境重建时精确恢复
- **安装源三选一** — 清华 / 阿里云 / 中科大镜像，规避官方源网络问题
- **端口自定义** — 默认 `18088`，向导与控制台均可修改；旧默认端口 `19091` 冲突时自动升迁自愈
- **预压缩传输** — 控制台静态资源按 `Accept-Encoding` 协商发送 brotli/gzip，52KB 控制台实际传输约 11KB

关于 QwenPaw 本体：多通道对话（微信、QQ、钉钉、飞书、Discord、Telegram 等）、多智能体协作、定时任务、Skills 无限扩展，支持 Ollama 等本地大模型完全离线运行，数据全部留在本地。由 [AgentScope](https://github.com/agentscope-ai) 团队开发。

## 安装引导

### 前置要求

- 飞牛 NAS 系统 **≥ 1.0.0**
- 依赖应用 **nodejs_v24** 与 **python312** 由系统在安装时自动拉起，无需手动安装
- 如需使用 Ollama 本地模型，请先在飞牛应用商店安装 **Ollama** 应用

### 安装步骤

1. 从 [Releases](https://github.com/yaozy2020/QwenPaw-for-fnOS/releases) 下载最新 `.fpk` 安装包
2. 飞牛应用商店 →「手动安装」→ 上传 `.fpk` 文件
3. 按向导完成四步配置：

| 向导步骤 | 配置项 | 说明 |
|:--|:--|:--|
| 安装源设置 | PyPI 安装源 | 内核从该镜像在线下载，国内推荐**清华源** |
| | 首发安装通道 | Stable（生产推荐）/ Beta（抢先体验） |
| Web 端口设置 | 服务端口 | 默认 `18088`，可改；填 `19091` 会自动迁到 `18088` |
| 日志级别 | info/debug/warning/error | 常规使用选 `info` |

4. 等待安装完成——内核 wheel（约 40MB）及其依赖需要从 PyPI 下载，**进度在 55% 附近停留较久属正常现象**

安装完成后，QwenPaw 内核在服务端口（默认 **18088**）启动；飞牛桌面会出现**两个图标**：QwenPaw（本体 Web 端）与 QwenPaw 控制台（运维面板）。

## 升级与版本管理

| 操作 | 入口 | 对内核的影响 |
|:--|:--|:--|
| **外壳升级**（控制台/脚本/图标优化等） | 应用商店手动安装新版 `.fpk` | **无**——内核版本保持不变（版本双轨制） |
| **内核升级/降级** | 控制台「版本管理」 | 显式选择目标版本或 latest/beta，成功后自动重启服务 |

> 内核降级有风险提示：旧版本可能无法识别新版本的配置字段与记忆结构，请谨慎操作。
> 升级内核建议先用控制台的「配置备份」下载数据备份。

## 卸载

卸载向导可选**保留应用数据**（重装后无缝恢复记忆与配置，推荐）或**彻底删除**（清空所有记忆、工作区与运行时环境）。

## 项目结构

```
QwenPaw-for-fnOS/
├── app/
│   ├── ui/                   # 运维控制台后端（api.cgi/index.cgi）与桌面图标
│   └── www/index.html        # 极简控制台（单文件零构建，提交即产物）
├── cmd/                      # 生命周期回调（install/upgrade/uninstall/config/main）
├── config/                   # 权限与资源配置
├── wizard/                   # 安装/升级/卸载/配置向导
├── scripts/                  # preflight、版本同步、AUDIT 生成等工程脚本
├── ui-fndesign/              # 旧版 Vue 控制台（遗留，未参与打包）
├── manifest                  # 应用清单（版本 SSOT）
├── build.sh                  # 零构建打包脚本（预压缩 + fnpack）
└── ICON.PNG / ICON_256.PNG   # 应用图标
```

## 版本历史

### v1.0.4（当前）

- 取消旧版数据自动迁移逻辑及相关文案
- 修复内核 2.3.0b2+ 在 fnOS 的启动崩溃（注入 `QWENPAW_DEPENDENCY_STATE_DIR`）
- 版本钉全路径闭环：外壳升级、环境重建均不改变内核版本

### v1.0.3

- 应用说明、README 安装引导、升级向导文案对齐实际功能
- 明确升级向导中 PyPI 镜像仅在运行环境重建时使用

### v1.0.2

- 修复外壳升级隐式变更内核版本的问题（v1.0.1 装包后内核被自动升级至 2.3.0b2、因上游启动锁 bug 失败的根因修复）
- 新增内核版本钉，`upgrade_callback` 遵守版本双轨制

### v1.0.1

- 控制台性能优化：预压缩协商传输（brotli/gzip）、轮询后台暂停
- 图标扁平化（去内层蓝盘，对齐 DSH 风格）并量化，包体 246K → 143K
- CI/Release 工作流对齐零构建架构

### v1.0.0

- 独立发版：确立外壳/内核版本双轨制
- DSH 风格极简控制台（双模态、版本自选、降级保护）
- 自定义端口支持

## 常见问题

### 安装进度卡在 55%？

正常现象。此时正在从 PyPI 镜像下载内核 wheel（约 40MB）及依赖并创建虚拟环境，视网络情况可能持续数分钟。

### 外壳升级会把 QwenPaw 内核也升级吗？

不会。外壳与内核版本双轨，外壳升级只更新控制台与生命周期脚本，内核版本原样保留（由版本钉保证）。内核升级请走控制台「版本管理」。

### 端口 19091 被占用或想换端口？

向导与控制台均可修改服务端口；安装时填入 `19091` 会自动改用 `18088`（冲突自愈）。运行中修改端口请在控制台「端口设置」操作。

### 使用 Ollama 时 API Key 怎么填？

连接 Ollama 时 API Key 字段不能为空，填写任意非空字符串（如 `ollama`）即可。

## 交流与反馈

- **GitHub Issues**：[提交反馈](https://github.com/yaozy2020/QwenPaw-for-fnOS/issues)

## 相关链接

| 项目 | 地址 |
|:--|:--|
| QwenPaw 本体 | [agentscope-ai/QwenPaw](https://github.com/agentscope-ai/QwenPaw) |
| AgentScope 团队 | [agentscope-ai](https://github.com/agentscope-ai) |
| 飞牛 NAS | [fnnas.com](https://www.fnnas.com/) |

## 许可证

本项目基于 [Apache-2.0](LICENSE) 许可证开源；QwenPaw 原项目同样采用 Apache-2.0 许可证。
