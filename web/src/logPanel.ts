import { nextTick, shallowReactive } from "vue";
import { createPollingLifecycle } from "./pollingLifecycle";
import type { ApiClient, LogEntry } from "./api";

const LOG_PAGE_SIZE = 100;
const LOG_WINDOW_MAX = 2000;
const LOG_REFRESH_MS = 15000;

interface LogPanelOptions {
  api: Pick<ApiClient, "logs" | "clearLogs">;
  showToast(message: string): void;
  confirm(options: { title: string; message: string; type: "warn" | "primary" | "danger"; confirmText: string }): Promise<boolean>;
  errorMessage(error: unknown): string;
  visible(): boolean;
}

function logKey(entry: LogEntry): string {
  return typeof entry === "string" ? `s:${entry}` : `o:${JSON.stringify(entry)}`;
}

function sameLogEntry(left: LogEntry, right: LogEntry): boolean {
  return left === right || logKey(left) === logKey(right);
}

// KMP O(n) 最长后缀-前缀重叠匹配，避免满窗口轮换时 O(n^2) 切片与序列化
function mergeLogWindow(existing: LogEntry[], incoming: LogEntry[]): LogEntry[] {
  if (!existing.length) return incoming.slice(-LOG_WINDOW_MAX);
  if (!incoming.length) return existing;
  const maxOverlap = Math.min(existing.length, incoming.length);
  const pattern = new Array<string>(maxOverlap);
  for (let i = 0; i < maxOverlap; i += 1) pattern[i] = logKey(incoming[i]);

  const lps = new Int32Array(maxOverlap);
  for (let i = 1, len = 0; i < maxOverlap;) {
    if (pattern[i] === pattern[len]) {
      lps[i++] = ++len;
    } else if (len > 0) {
      len = lps[len - 1];
    } else {
      lps[i++] = 0;
    }
  }

  let matched = 0;
  const start = existing.length - maxOverlap;
  for (let i = start; i < existing.length; i += 1) {
    const key = logKey(existing[i]);
    while (matched > 0 && pattern[matched] !== key) matched = lps[matched - 1];
    if (pattern[matched] === key) matched += 1;
  }

  if (matched === 0) return incoming.slice(-LOG_WINDOW_MAX);
  if (matched === incoming.length) return existing;
  return [...existing, ...incoming.slice(matched)].slice(-LOG_WINDOW_MAX);
}

export type LogLevelFilter = "all" | "warn" | "error";

export function logEntryText(entry: LogEntry): string {
  return typeof entry === "string" ? entry : entry.message || JSON.stringify(entry);
}

export function matchesLogFilter(entry: LogEntry, level: LogLevelFilter, query: string, normalized = false): boolean {
  const text = logEntryText(entry);
  if (level === "error" && !/ERROR|Error|failed|失败/.test(text)) return false;
  if (level === "warn" && !/ERROR|Error|failed|失败|WARN|Warning/.test(text)) return false;
  const keyword = normalized ? query : query.trim().toLowerCase();
  if (keyword && !text.toLowerCase().includes(keyword)) return false;
  return true;
}

export function createLogPanel({ api, showToast, confirm, errorMessage, visible }: LogPanelOptions) {
  let logLines: LogEntry[] = [];
  let renderedLines: LogEntry[] | null = null;
  let logClearGeneration = 0;
  let liveLogFetchSeq = 0;
  let followLatest = true;
  let disconnectDom: (() => void) | null = null;
  const view = shallowReactive({
    lines: [] as LogEntry[],
    exportBusy: false,
    error: '',
    level: 'all' as LogLevelFilter,
    query: '',
  });

  let inLogs: boolean | undefined;
  let autoRefresh = true;

  function render(): void {
    const terminal = document.getElementById("logTerminal");
    if (!terminal) return;
    if (renderedLines === logLines || renderedLines && renderedLines.length === logLines.length
      && renderedLines.every((entry, index) => sameLogEntry(entry, logLines[index]))) return;
    followLatest = terminal.scrollHeight - terminal.clientHeight - terminal.scrollTop <= 24;
    const shouldScroll = followLatest;
    view.lines = logLines;
    renderedLines = logLines;
    const autoScroll = document.getElementById("autoScrollCheck") as HTMLInputElement | null;
    if (autoScroll?.checked && shouldScroll) void nextTick(() => { if (followLatest) terminal.scrollTop = terminal.scrollHeight; });
  }

  async function refresh(): Promise<void> {
    if (document.hidden || !visible()) return;
    const generation = logClearGeneration;
    const mySeq = ++liveLogFetchSeq;
    try {
      const result = await api.logs(logLines.length ? LOG_PAGE_SIZE : LOG_WINDOW_MAX);
      if (document.hidden || !visible() || generation !== logClearGeneration || mySeq !== liveLogFetchSeq) return;
      const data = result.data;
      if (result.ok && data.success && Array.isArray(data.logs)) {
        view.error = '';
        logLines = mergeLogWindow(logLines, data.logs);
        render();
      } else view.error = "读取日志失败，可重试。";
    } catch (error) {
      if (mySeq === liveLogFetchSeq && generation === logClearGeneration && visible()) view.error = "读取日志失败，可重试。";
    }
  }

  async function clear(): Promise<void> {
    const ok = await confirm({
      title: "删除服务日志",
      message: "这将删除后端保存的服务日志，而不仅仅清空当前显示。删除后无法恢复，确认继续？",
      type: "danger",
      confirmText: "确认删除日志",
    });
    if (!ok) return;
    try {
      const result = await api.clearLogs();
      if (!result.ok || result.data.success === false) throw new Error(result.data.message || `删除日志失败 (${result.status})`);
      ++logClearGeneration;
      ++liveLogFetchSeq;
      logLines = [];
      followLatest = true;
      render();
      showToast("已请求清理服务日志");
    } catch (error) {
      showToast(`删除日志失败: ${errorMessage(error)}`);
    }
  }

  async function exportLogs(): Promise<void> {
    if (view.exportBusy) return;
    view.exportBusy = true;
    try {
      const result = await api.logs(LOG_WINDOW_MAX);
      if (!result.ok || !result.data.success || !Array.isArray(result.data.logs)) {
        throw new Error(result.data.message || `读取日志失败 (${result.status})`);
      }
      const content = result.data.logs.map(logEntryText).join("\n") + "\n";
      const url = URL.createObjectURL(new Blob([content], { type: "text/plain;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = `qwenpaw-${new Date().toISOString().replace(/[:.]/g, "-")}.log`;
      link.hidden = true;
      document.body.appendChild(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      showToast(`已导出 ${result.data.logs.length} 行日志`);
    } catch (error) {
      showToast(`导出日志失败: ${errorMessage(error)}`);
    } finally {
      view.exportBusy = false;
    }
  }

  const polling = createPollingLifecycle({ refresh, intervalMs: LOG_REFRESH_MS });

  function viewChanged(): void {
    const next = visible();
    if (next === inLogs) return;
    inLogs = next;
    ++liveLogFetchSeq;
    if (!next) { polling.stop(); return; }
    if (autoRefresh) polling.start();
    void polling.refresh();
  }

  function setAutoRefresh(enable: boolean): void {
    autoRefresh = enable;
    if (enable && visible()) polling.start();
    else polling.stop();
  }

  async function copyLogs(entries?: LogEntry[]): Promise<void> {
    const source = entries ?? (
      view.level === "all" && !view.query.trim()
        ? view.lines
        : view.lines.filter(entry => matchesLogFilter(entry, view.level, view.query))
    );
    if (!source.length) {
      showToast("当前无日志可复制");
      return;
    }
    const text = source.map(logEntryText).join("\n");
    try {
      if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
        await navigator.clipboard.writeText(text);
      } else {
        const area = document.createElement("textarea");
        area.value = text;
        area.setAttribute("readonly", "");
        area.style.position = "fixed";
        area.style.opacity = "0";
        document.body.appendChild(area);
        area.select();
        const copied = document.execCommand("copy");
        area.remove();
        if (!copied) throw new Error("浏览器不支持剪贴板写入");
      }
      showToast(`已复制 ${source.length} 行日志`);
    } catch (error) {
      showToast(`复制日志失败: ${errorMessage(error)}`);
    }
  }

  function connect(): void {
    disconnectDom?.();
    const terminal = document.getElementById("logTerminal");
    const onScroll = () => {
      if (!terminal) return;
      followLatest = terminal.scrollHeight - terminal.clientHeight - terminal.scrollTop <= 24;
    };
    terminal?.addEventListener("scroll", onScroll, { passive: true });
    const autoScroll = document.getElementById("autoScrollCheck") as HTMLInputElement | null;
    const onAutoScrollChange = () => {
      if (autoScroll?.checked && terminal) {
        followLatest = true;
        terminal.scrollTop = terminal.scrollHeight;
      }
    };
    autoScroll?.addEventListener("change", onAutoScrollChange);
    let observer: ResizeObserver | null = null;
    if (terminal && typeof ResizeObserver === "function") {
      observer = new ResizeObserver(() => {
        if (followLatest && autoScroll?.checked && terminal.clientHeight > 0) {
          terminal.scrollTop = terminal.scrollHeight;
        }
      });
      observer.observe(terminal);
    }
    disconnectDom = () => {
      terminal?.removeEventListener("scroll", onScroll);
      autoScroll?.removeEventListener("change", onAutoScrollChange);
      observer?.disconnect();
    };
  }
  return {
    view, connect, refresh,
    watching: () => autoRefresh && visible() && !document.hidden,
    notifyChange: () => { if (autoRefresh && visible() && !document.hidden) void polling.refresh(); },
    viewChanged,
    clear,
    destroy: () => {
      disconnectDom?.();
      disconnectDom = null;
      polling.destroy();
    },
    setAutoRefresh,
    exportLogs,
    copyLogs,
  };
}
