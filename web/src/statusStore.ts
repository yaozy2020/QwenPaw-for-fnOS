import type { StatusResponse } from "./api";

// 状态快照类型与格式化工具(Preact 版:纯数据契约,无框架绑定)。
// 渲染契约:vanilla 轮询(fetchStatus 等)每拍以最新数据调 render() 重渲染,
// Preact 负责 diff —— 行为语义(节奏/隐藏页暂停/旁路副作用)全部留在 vanilla。
export interface StatusData {
  // undefined = 尚未取得首个快照；null = 后端不可达、安装状态未知
  // 三态必须区分:把"未知"写成 false 会被下游当成"确定未安装"
  installed: boolean | null | undefined;
  running: boolean;
  pid: string;
  startAt: number | null; // epoch 秒
  cpu: number | null; // 占总 CPU 百分比(监督者 300ms 采样)
  memKB: number | null; // RSS
  degraded?: boolean; // true = 后端不可达(502/网络错误/会话过期),非"未安装"
  degradedDetail?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonnegativeNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

// SSE 必须是 Rust /status 的完整快照，不能把错误对象或部分字段当作已停止/公开访问。
// 不用 install 的 seq/instance 推断任务归属，安装窗口仍独立确认受理身份。
export function isStatusSnapshot(value: unknown): value is StatusResponse {
  if (!isRecord(value)) return false;
  return value.success === true
    && typeof value.installed === "boolean"
    && typeof value.running === "boolean"
    && (typeof value.pid === "string" && /^\d+$/.test(value.pid)
      || typeof value.pid === "number" && Number.isSafeInteger(value.pid) && value.pid >= 0)
    && nonnegativeNumber(value.startAt) && Number.isSafeInteger(value.startAt)
    && nonnegativeNumber(value.cpu)
    && nonnegativeNumber(value.memKB) && Number.isSafeInteger(value.memKB)
    && typeof value.version === "string"
    && typeof value.port === "number" && Number.isInteger(value.port) && value.port > 0 && value.port <= 65535
    && typeof value.authEnabled === "boolean"
    && typeof value.authStatus === "string"
    && ["unknown", "uninitialized", "protected", "disabled"].includes(value.authStatus)
    && isRecord(value.install) && typeof value.install.running === "boolean";
}

export function formatUptime(seconds: number | null): string {
  if (!seconds || seconds <= 0) return "-";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  // 进程运行时长须显示到秒:刚重启的 59 秒内只显示"0分钟"会被误读为卡住
  if (days > 0) return `${days}天 ${hours}小时 ${minutes}分`;
  if (hours > 0) return `${hours}小时 ${minutes}分`;
  if (minutes > 0) return `${minutes}分 ${secs}秒`;
  return `${secs}秒`;
}

export function formatMem(kb: number | null): string {
  if (!kb || kb <= 0) return "-";
  if (kb >= 1024 * 1024) return (kb / 1024 / 1024).toFixed(2) + " GB";
  if (kb >= 1024) return (kb / 1024).toFixed(1) + " MB";
  return `${kb} KB`;
}
