# 会话交接：QwenPaw-for-fnOS 管理层整改

- 交接日期：2026-11-04。
- 项目：仓库根目录。
- 本文记录本地最终候选与实际验证结果，并将其与尚未完成的 fnOS 真机、上游和发布验收明确区分。

## 当前状态

- P0–P5 本地实现及安全复核修复已完成；生产管理层已迁入 Rust/native CGI 薄桥，QwenPaw 本体未修改。P4 fnOS 统一网关/真机验收仍阻塞。
- 初审整改已完成：修复崩溃后的 service stop/restart、Committed 后 pin/journal 写失败、Web stop/restart/uninstall 停止门禁、UDS `seq + instance` 归属；清理 TS 非空断言和文档本机路径。
- 最新本地包（1.1.1 RC1）：`build/output/qwenpaw-1.1.1-1-x86.fpk`；大小 `2,071,727` 字节；SHA256 `ec6850e3196f69545e66d649c4ce15a59b70529c6dc1ab45845fcc8d12ed2e0a`。
- 包内 supervisor SHA256：`075ea6f48ab9c617aae9888eea63565e06ee5703e2afcdac5aa307697107c607`；测试 helper SHA256：`dd59f19e53204fa9f81d4cab097c0d89af002a95d595cc7247cfb84584521c91`。
- 工作树保留既有改动；未 reset、clean 或覆盖其他改动。

## 已通过的本地验证

初审整改后的最终工作树通过：Rust 222+13、Node 30、严格 Clippy/typecheck、preflight、完整 STRICT_CI lint、audit、自测、API/lifecycle 120 并发、七点恢复、native API 15、index 22、platform 11、真实 venv 契约及 Chromium 1 项。原始 SIGKILL→service stop/restart 场景也已在临时 TRIM 根和动态端口下实测通过。

测试所用 release supervisor 与最新包内二进制摘要一致；最新包已跑构建内审计，但未另行解包重跑全部包内套件。本文档收尾更新不改变实现或产物摘要；`python3 scripts/gen-audit.py --check` 与 `git diff --check` 在文档更新后另行复核。

## 尚未验证的外部/平台事项

以下事项仍未取得实际验证证据：

- fnOS 真机统一网关、gatewaySocket/UDS 路径、登录态与鉴权；本机私有 `supervisor-control.sock` 不等于 fnOS `gatewaySocket`。
- fnOS 回调执行窗口（包括卸载回调时原生程序是否仍可执行）及实际回调时序/重试行为。
- manifest 声明的最低 fnOS 版本兼容性。
- 实际平台停止预算/停止时限。
- 真实 PyPI 上游包的下载及上游运行冒烟。
- WebKit 与 fnOS 实际 App 行为；Chromium 通过不替代这些验证。
- 真机升级/回退；回退尚未演练。

本地测试和候选包不能提供以上真机/上游事项的实际证据。

## 回退边界与操作状态

- 只有在未决事务已恢复完成、且目标旧包确认兼容当前持久化 schema/layout 后，才考虑回退到已知 SHA 的旧包；不兼容或无法确认时不得直接回退。真机回退未演练。
- 完整删除用户数据不可回滚；程序/包回退不等于用户数据回退。
- 未 commit、push、发布或部署；未操作真实数据。
