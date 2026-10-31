// 状态快照类型与格式化工具(Preact 版:纯数据契约,无框架绑定)。
// 渲染契约:vanilla 轮询(fetchStatus 等)每拍以最新数据调 render() 重渲染,
// Preact 负责 diff —— 行为语义(节奏/隐藏页暂停/旁路副作用)全部留在 vanilla。
export interface StatusData {
  installed: boolean | undefined; // undefined = 尚未取得首个快照
  running: boolean;
  pid: string;
  startAt: number | null; // epoch 秒
  cpu: number | null; // 占总 CPU 百分比(监督者 300ms 采样)
  memKB: number | null; // RSS
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
