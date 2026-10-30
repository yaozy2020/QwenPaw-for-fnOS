# QwenPaw for fnOS

[![Version](https://img.shields.io/badge/version-1.0.2-blue)](https://github.com/dustink66/com.dustinky.qwenpaw/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-green)](LICENSE)
[![fnOS](https://img.shields.io/badge/fnOS-%E2%89%A5%200.9.21-orange)](https://www.fnnas.com/)

将 [QwenPaw](https://github.com/agentscope-ai/QwenPaw) 一键部署到飞牛 NAS 的应用包。

## 简介

QwenPaw 是一款部署在你自己的 NAS 上的个人 AI 助理。它不只是聊天机器人 — 你可以通过微信、QQ、钉钉、飞书、Discord、Telegram 等多通道与它交互，配置定时任务让它自动执行工作，通过 Skills 无限扩展它的能力。所有数据完全存储在本地，无需依赖任何第三方云服务。

由 [AgentScope](https://github.com/agentscope-ai) 团队基于 AgentScope、AgentScope Runtime 与 ReMe 构建，支持本地大模型完全离线运行。

## 核心功能

- **多通道对话** — 支持微信、QQ、钉钉、飞书、Discord、Telegram、iMessage 等，一处部署，全平台接入
- **多智能体协作** — 独立配置多个 Agent，各自拥有独立的记忆和技能，可互相通信协作完成复杂任务
- **定时任务** — 设置 Cron 定时任务，让 QwenPaw 在指定时间自动执行工作
- **Skills 扩展** — 能力由 Skills 决定，支持无限扩展，社区持续贡献新技能
- **本地优先** — 支持 Ollama 等本地大模型，完全离线也能工作，数据不外泄
- **多层安全防护** — 内置沙箱执行、会话隔离、权限控制等安全机制

## 快速开始

### 前置要求

- 飞牛 NAS 系统 **≥ 0.9.21**
- 如需使用内置 Ollama 驱动，请先在飞牛应用商店安装 **Ollama** 应用

### 安装

1. 从 [Releases](https://github.com/dustink66/com.dustinky.qwenpaw/releases) 下载最新 `.fpk` 安装包
2. 在飞牛应用商店中选择「手动安装」，上传 `.fpk` 文件
3. 安装过程中选择日志级别（默认 `info`，调试可选 `debug`）
4. 等待安装完成（进度条在 55% 左右可能停留较长时间，属于正常现象）

安装完成后，QwenPaw 将在 **端口 19091** 上启动 Web 管理界面。

### 配置说明

| 配置项 | 说明 | 可选值 |
|--------|------|--------|
| 日志级别 | 控制日志输出详细程度 | `info` / `debug` / `warning` / `error` |

### 依赖说明

安装包会自动处理以下依赖：

- **Python 3.12** — Python 运行环境
- **Node.js v24** — 前端构建和运行时环境
- **Ollama**（可选）— 如需使用本地模型，请手动安装

## 项目结构

```
com.dustinky.qwenpaw/
├── app/
│   ├── qwenpaw/code/        # QwenPaw 源码（从上游同步）
│   ├── ui/                   # API 网关和 Web UI 资源
│   └── www/                  # 前端静态资源
├── cmd/
│   ├── main                  # 主程序入口（启动/停止/状态管理）
│   ├── install_callback      # 安装后回调（创建 venv、安装依赖）
│   ├── upgrade_callback      # 升级后回调
│   ├── uninstall_callback    # 卸载回调
│   └── config_callback       # 配置变更回调
├── config/
│   └── resource              # 数据共享目录权限配置
├── wizard/
│   └── config                # 安装向导配置
├── manifest                  # 应用清单（名称、版本、描述等）
├── ICON.png / ICON_256.png   # 应用图标
└── LICENSE                   # Apache-2.0 许可证
```

## 版本历史

### v1.1.10（当前版本）

- 优化控制台功能和 UI
- Node.js 依赖版本升级到 v24
- 新增 Agent 子代理（spawn_subagent）功能
- 新增打开目录标签页，可直接引用本地项目
- 新增腾讯元宝 Bot 渠道
- 新增飞书消息话题回复
- 新增 OpenRouter 应用归因
- 新增动态上传限制配置
- 修复桌面端内置 CLI、Windows Git 控制台等问题
- 修复控制台聊天输入、代码展示等多项问题
- 修复 OneBot WebSocket 断连重启
- 修复技能更新稳定性
- 宠物自动安装依赖等多项优化

## 常见问题

### 安装进度卡在 55%？

这是正常现象。安装过程中需要创建 Python 虚拟环境并从 PyPI 安装 QwenPaw 及其依赖，此阶段耗时较长，请耐心等待。

### 使用 Ollama 时 API Key 怎么填？

连接 Ollama 时 API Key 字段不能为空，可以填写任意非空字符串（如 `ollama`）。

### 如何更新？

在飞牛应用商店中找到 QwenPaw，点击更新即可。更新过程中可选择保留现有运行环境以加快升级速度。

### 如何访问管理界面？

安装完成后，在飞牛桌面点击 QwenPaw 图标，或在浏览器中访问 `http://<NAS_IP>:19091`。

## 交流与反馈

- **GitHub Issues**：[提交反馈](https://github.com/dustink66/com.dustinky.qwenpaw/issues)

## 相关链接

| 项目 | 地址 |
|------|------|
| QwenPaw 项目主页 | [agentscope-ai/QwenPaw](https://github.com/agentscope-ai/QwenPaw) |
| AgentScope 团队 | [agentscope-ai](https://github.com/agentscope-ai) |
| 飞牛 NAS 官网 | [fnnas.com](https://www.fnnas.com/) |
| 作者主页 | [dustinky.com](https://www.dustinky.com) |

## 许可证

本项目基于 [Apache-2.0](LICENSE) 许可证开源。

QwenPaw 原项目同样采用 Apache-2.0 许可证。