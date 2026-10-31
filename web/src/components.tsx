// Preact 视图组件:纯函数 of 数据,不含轮询与业务时序。
// 插值默认转义 —— 日志/版本号等外部数据不再依赖手工 escapeHtml(r7 同族风险点)。
import { formatUptime, formatMem, type StatusData } from "./statusStore";

export function StatusCardView({ data }: { data: StatusData }) {
  const notInstalled = data.installed === false;
  const running = !notInstalled && data.running;
  const stateText = notInstalled ? "内核未安装" : running ? "运行中 (Active)" : "已停止 (Stopped)";
  const stateColor = notInstalled ? "var(--text-sub)" : running ? "var(--success)" : "var(--danger)";
  const uptime = running && data.startAt ? formatUptime(Math.floor(Date.now() / 1000) - data.startAt) : "-";
  const rows: [string, string][] = [
    ["服务状态", stateText],
    ["进程 PID", running ? String(data.pid || "-") : "-"],
    ["运行时长", uptime],
    ["CPU 占用", running && typeof data.cpu === "number" ? data.cpu.toFixed(1) + "%" : "-"],
    ["内存占用", running ? formatMem(data.memKB) : "-"],
  ];
  return (
    <>
      {rows.map(([label, val]) => (
        <div class="info-row" key={label}>
          <span class="info-label">{label}</span>
          <span class="info-val" style={label === "服务状态" ? { color: stateColor } : undefined}>{val}</span>
        </div>
      ))}
    </>
  );
}

export function LogLines({ lines, empty, success = false }: { lines: (string | { message?: string })[]; empty: string; success?: boolean }) {
  const items = lines.map(item => (typeof item === "string" ? item : (item.message || JSON.stringify(item))));
  if (!items.length) return <span style="color:var(--text-muted);">{empty}</span>;
  return (
    <>
      {items.map((msg, i) => {
        let cls = "log-info";
        if (msg.includes("ERROR") || msg.includes("Error") || msg.includes("failed") || msg.includes("失败")) cls = "log-error";
        else if (msg.includes("WARN") || msg.includes("Warning")) cls = "log-warn";
        else if (success && (msg.includes("成功") || msg.includes("Successfully"))) cls = "log-success";
        return <div class={cls} key={i}>{msg}</div>;
      })}
    </>
  );
}

export function MirrorOptions({ options, value, onPick }: {
  options: { value: string; label: string }[];
  value: string;
  onPick: (v: string) => void;
}) {
  return (
    <>
      {options.map(m => (
        <div class="history-item" key={m.value || "official"}
          style={m.value === value ? "color:var(--primary);" : ""}
          onClick={() => onPick(m.value)}>
          <span>{m.label}</span>
          {m.value === value ? <span style="color:var(--primary);">✓</span> : ""}
        </div>
      ))}
    </>
  );
}

export function VersionCardsView({ beta, stable, selected, onPick }: {
  beta: string; stable: string; selected: string; onPick: (v: string) => void;
}) {
  const historySelected = selected !== beta && selected !== stable;
  const Card = ({ ver, tag, cls, active }: { ver: string; tag: string; cls: string; active: boolean }) => (
    <div class={`ver-card-item ${active ? "active" : ""}`} onClick={() => onPick(ver)}>
      <div class="ver-info-left">
        <span class="ver-number">v{ver}</span>
        <span class={`ver-tag-pill ${cls}`}>{tag}</span>
      </div>
      <div class="custom-radio"></div>
    </div>
  );
  return (
    <>
      {historySelected && <Card ver={selected} tag="自选历史版本" cls="history" active />}
      <Card ver={beta} tag="最新测试 Beta" cls="beta" active={selected === beta} />
      <Card ver={stable} tag="推荐稳定 Stable" cls="stable" active={selected === stable} />
    </>
  );
}

export function HistoryItems({ items, onPick }: {
  items: { version: string; is_beta: boolean }[];
  onPick: (v: string) => void;
}) {
  return (
    <>
      {items.map(item => (
        <div class="history-item" key={item.version} onClick={() => onPick(item.version)}>
          <span style="font-family:monospace; font-weight:600;">v{item.version}</span>
          <span class={`ver-tag-pill ${item.is_beta ? "beta" : "stable"}`} style="font-size:10px;">{item.is_beta ? "Beta" : "稳定版"}</span>
        </div>
      ))}
    </>
  );
}

// 命令式渲染入口:main.ts(.ts,无 JSX)每拍调用,由这里承担 JSX。
import { render } from "preact";

export function renderStatusCard(data: StatusData, el: HTMLElement) {
  render(<StatusCardView data={data} />, el);
}

export function renderLogLines(lines: (string | { message?: string })[], el: HTMLElement, empty: string, success = false) {
  render(<LogLines lines={lines} empty={empty} success={success} />, el);
}

export function renderMirrorOptions(options: { value: string; label: string }[], value: string, onPick: (v: string) => void, el: HTMLElement) {
  render(<MirrorOptions options={options} value={value} onPick={onPick} />, el);
}

export function renderVersionCards(beta: string, stable: string, selected: string, onPick: (v: string) => void, el: HTMLElement) {
  render(<VersionCardsView beta={beta} stable={stable} selected={selected} onPick={onPick} />, el);
}

export function renderHistoryItems(items: { version: string; is_beta: boolean }[], onPick: (v: string) => void, el: HTMLElement) {
  render(<HistoryItems items={items} onPick={onPick} />, el);
}
