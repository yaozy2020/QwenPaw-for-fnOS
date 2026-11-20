import type { StatusResponse } from "./api";

// 状态快照与格式化独立于 UI；组件不拥有轮询和订阅。
export interface StatusData {
  // undefined = 尚未取得首个快照；null = 后端不可达、安装状态未知
  // 三态必须区分:把"未知"写成 false 会被下游当成"确定未安装"
  installed: boolean | null | undefined;
  running: boolean;
  pid: string;
  startAt: number | null; // epoch 秒
  cpu: number | null; // 相邻进程采样的 CPU 百分比，首个样本暂无差值
  memKB: number | null; // RSS
  degraded?: boolean; // true = 后端不可达(502/网络错误/会话过期),非"未安装"
  degradedDetail?: string;
  recoveryReason?: string;
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
    && (value.cpu === null || nonnegativeNumber(value.cpu))
    && nonnegativeNumber(value.memKB) && Number.isSafeInteger(value.memKB)
    && typeof value.version === "string"
    && typeof value.port === "number" && Number.isInteger(value.port) && value.port > 0 && value.port <= 65535
    && typeof value.authEnabled === "boolean"
    && typeof value.authStatus === "string"
    && ["unknown", "uninitialized", "protected", "disabled"].includes(value.authStatus)
    && isRecord(value.install) && typeof value.install.running === "boolean";
}

export function formatUptime(seconds: number | null): string {
  if (seconds === null || seconds < 0) return "-";
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);
  if (hours > 0 || days > 0) {
    const units = days ? [hours, minutes] : [hours, minutes, secs];
    const clock = units.map(n => String(n).padStart(2, '0')).join(':');
    return days ? `${days}天 ${clock}` : clock;
  }
  if (minutes > 0) return `${minutes}分 ${secs}秒`;
  return `${secs}秒`;
}

export function formatMem(kb: number | null): string {
  if (!kb || kb <= 0) return "-";
  if (kb >= 1024 * 1024) return (kb / 1024 / 1024).toFixed(2) + " GB";
  if (kb >= 1024) return (kb / 1024).toFixed(1) + " MB";
  return `${kb} KB`;
}
