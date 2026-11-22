import { nextTick, shallowReactive } from "vue";
import { createPollingLifecycle } from "./pollingLifecycle";
import type { ApiClient, LogEntry } from "./api";
import { t } from "./i18n";

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
  const raw = shallowReactive({
    lines: [] as LogEntry[],
    exportBusy: false,
    hasError: false,
    level: 'all' as LogLevelFilter,
    query: '',
    autoScroll: true,
  });
  const view = {
    get lines() { return raw.lines; },
    set lines(value: LogEntry[]) { raw.lines = value; },
    get exportBusy() { return raw.exportBusy; },
    set exportBusy(value: boolean) { raw.exportBusy = value; },
    get error() { return raw.hasError ? t("logs.readError") : ""; },
    get level() { return raw.level; },
    set level(value: LogLevelFilter) { raw.level = value; },
    get query() { return raw.query; },
    set query(value: string) { raw.query = value; },
    get autoScroll() { return raw.autoScroll; },
    set autoScroll(value: boolean) { raw.autoScroll = value; },
  };

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
    if (view.autoScroll && shouldScroll) void nextTick(() => { if (followLatest) terminal.scrollTop = terminal.scrollHeight; });
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
        raw.hasError = false;
        logLines = mergeLogWindow(logLines, data.logs);
        render();
      } else raw.hasError = true;
    } catch (error) {
      if (mySeq === liveLogFetchSeq && generation === logClearGeneration && visible()) raw.hasError = true;
    }
  }

  async function clear(): Promise<void> {
    const ok = await confirm({
      title: t("logs.clearTitle"),
      message: t("logs.clearMsg"),
      type: "danger",
      confirmText: t("logs.clearConfirm"),
    });
    if (!ok) return;
    try {
      const result = await api.clearLogs();
      if (!result.ok || result.data.success === false) throw new Error(result.data.message || t("logs.clearFailedStatus", { status: result.status }));
      ++logClearGeneration;
      ++liveLogFetchSeq;
      logLines = [];
      followLatest = true;
      render();
      showToast(t("logs.clearedToast"));
    } catch (error) {
      showToast(t("logs.clearFailed", { error: errorMessage(error) }));
    }
  }

  async function exportLogs(): Promise<void> {
    if (view.exportBusy) return;
    view.exportBusy = true;
    try {
      const result = await api.logs(LOG_WINDOW_MAX);
      if (!result.ok || !result.data.success || !Array.isArray(result.data.logs)) {
        throw new Error(result.data.message || t("logs.readFailedStatus", { status: result.status }));
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
      showToast(t("logs.exportedToast", { count: result.data.logs.length }));
    } catch (error) {
      showToast(t("logs.exportFailed", { error: errorMessage(error) }));
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
      showToast(t("logs.nothingToCopy"));
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
        if (!copied) throw new Error(t("logs.clipboardUnsupported"));
      }
      showToast(t("logs.copiedToast", { count: source.length }));
    } catch (error) {
      showToast(t("logs.copyFailed", { error: errorMessage(error) }));
    }
  }

  function setAutoScroll(enabled: boolean): void {
    view.autoScroll = enabled;
    const terminal = document.getElementById("logTerminal");
    if (enabled && terminal) {
      followLatest = true;
      terminal.scrollTop = terminal.scrollHeight;
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
    let observer: ResizeObserver | null = null;
    if (terminal && typeof ResizeObserver === "function") {
      observer = new ResizeObserver(() => {
        if (followLatest && view.autoScroll && terminal.clientHeight > 0) {
          terminal.scrollTop = terminal.scrollHeight;
        }
      });
      observer.observe(terminal);
    }
    disconnectDom = () => {
      terminal?.removeEventListener("scroll", onScroll);
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
    setAutoScroll,
    exportLogs,
    copyLogs,
  };
}
