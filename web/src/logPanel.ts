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

function sameLogEntry(left: LogEntry, right: LogEntry): boolean {
  return left === right || JSON.stringify(left) === JSON.stringify(right);
}

function mergeLogWindow(existing: LogEntry[], incoming: LogEntry[]): LogEntry[] {
  if (!existing.length) return incoming.slice(-LOG_WINDOW_MAX);
  let overlap = Math.min(existing.length, incoming.length);
  while (overlap > 0) {
    const matches = existing.slice(-overlap).every((entry, index) => sameLogEntry(entry, incoming[index]));
    if (matches) break;
    overlap -= 1;
  }
  if (overlap === 0) return incoming.slice(-LOG_WINDOW_MAX);
  if (overlap === incoming.length) return existing;
  return [...existing, ...incoming.slice(overlap)].slice(-LOG_WINDOW_MAX);
}

function logText(entry: LogEntry): string {
  return typeof entry === "string" ? entry : entry.message || JSON.stringify(entry);
}

export function createLogPanel({ api, showToast, confirm, errorMessage, visible }: LogPanelOptions) {
  let logLines: LogEntry[] = [];
  let renderedLines: LogEntry[] | null = null;
  let logClearGeneration = 0;
  let liveLogFetchSeq = 0;
  let followLatest = true;
  const view = shallowReactive({ lines: [] as LogEntry[], exportBusy: false, error: '' });

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
      const content = result.data.logs.map(logText).join("\n") + "\n";
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

  function connect(): void {
    const terminal = document.getElementById("logTerminal");
    terminal?.addEventListener("scroll", () => {
      followLatest = terminal.scrollHeight - terminal.clientHeight - terminal.scrollTop <= 24;
    }, { passive: true });
    const autoScroll = document.getElementById("autoScrollCheck") as HTMLInputElement | null;
    autoScroll?.addEventListener("change", () => {
      if (autoScroll.checked && terminal) {
        followLatest = true;
        terminal.scrollTop = terminal.scrollHeight;
      }
    });
    if (terminal && typeof ResizeObserver === "function") {
      const observer = new ResizeObserver(() => {
        if (followLatest && autoScroll?.checked && terminal.clientHeight > 0) {
          terminal.scrollTop = terminal.scrollHeight;
        }
      });
      observer.observe(terminal);
    }
  }
  return {
    view, connect, refresh,
    watching: () => autoRefresh && visible() && !document.hidden,
    notifyChange: () => { if (autoRefresh && visible() && !document.hidden) void polling.refresh(); },
    viewChanged,
    clear,
    destroy: polling.destroy,
    setAutoRefresh,
    exportLogs,
  };
}
