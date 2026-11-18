import { renderLogLines } from "./components";
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

export function createLogPanel({ api, showToast, confirm, errorMessage }: LogPanelOptions) {
  let logLines: LogEntry[] = [];
  let renderedLines: LogEntry[] | null = null;
  let logClearGeneration = 0;
  let liveLogFetchSeq = 0;
  let followLatest = true;
  let exportBusy = false;

  function renderOverviewPreview(): void {
    const preview = document.getElementById("overviewLogPreview");
    if (!preview) return;
    const text = logLines.slice(-2).map(logText).join("\n") || "暂无日志";
    if (preview.innerText !== text) preview.innerText = text;
  }

  function render(): void {
    const terminal = document.getElementById("logTerminal");
    if (!terminal) return;
    if (renderedLines === logLines || renderedLines && renderedLines.length === logLines.length
      && renderedLines.every((entry, index) => sameLogEntry(entry, logLines[index]))) return;
    followLatest = terminal.scrollHeight - terminal.clientHeight - terminal.scrollTop <= 24;
    const shouldScroll = followLatest;
    renderLogLines(logLines, terminal, "（当前无日志输出）");
    renderedLines = logLines;
    renderOverviewPreview();
    const autoScroll = document.getElementById("autoScrollCheck") as HTMLInputElement | null;
    if (autoScroll?.checked && shouldScroll) terminal.scrollTop = terminal.scrollHeight;
  }

  async function refresh(): Promise<void> {
    if (document.hidden) return;
    const generation = logClearGeneration;
    const mySeq = ++liveLogFetchSeq;
    try {
      const result = await api.logs(logLines.length ? LOG_PAGE_SIZE : LOG_WINDOW_MAX);
      if (!result.ok || generation !== logClearGeneration || mySeq !== liveLogFetchSeq) return;
      const data = result.data;
      if (data.success && Array.isArray(data.logs)) {
        logLines = mergeLogWindow(logLines, data.logs);
        render();
      }
    } catch (error) {
      console.error("fetchLogs err:", error);
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
    if (exportBusy) return;
    exportBusy = true;
    const button = document.getElementById("exportLogsBtn") as HTMLButtonElement | null;
    if (button) {
      button.disabled = true;
      button.setAttribute("aria-busy", "true");
    }
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
      exportBusy = false;
      if (button) {
        button.disabled = false;
        button.removeAttribute("aria-busy");
      }
    }
  }

  const polling = createPollingLifecycle({ refresh, intervalMs: LOG_REFRESH_MS });

  function setAutoRefresh(enable: boolean): void {
    if (enable) polling.start();
    else polling.stop();
  }

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
  document.getElementById("exportLogsBtn")?.addEventListener("click", () => void exportLogs());

  return {
    refresh,
    clear,
    start: polling.start,
    stop: polling.stop,
    destroy: polling.destroy,
    setAutoRefresh,
    exportLogs,
  };
}
