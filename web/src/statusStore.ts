// 监督者 /status 快照的共享 store —— vanilla 世界与 Svelte 组件之间的契约:
// main.ts 的轮询(fetchStatus)负责取数与旁路副作用(徽章/提示/版本联动),
// 组件只按订阅声明式渲染。轮询节奏与隐藏页暂停语义留在 vanilla,行为不变。
import { writable } from "svelte/store";

export interface StatusData {
  installed: boolean | undefined; // undefined = 尚未取得首个快照
  running: boolean;
  pid: string;
  startAt: number | null; // epoch 秒
  cpu: number | null; // 占总 CPU 百分比(监督者 300ms 采样)
  memKB: number | null; // RSS
}

export const status = writable<StatusData>({
  installed: undefined,
  running: false,
  pid: "-",
  startAt: null,
  cpu: null,
  memKB: null,
});

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
