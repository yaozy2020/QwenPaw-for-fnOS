// Preact 视图组件:纯函数 of 数据,不含轮询与业务时序。
// 插值默认转义 —— 日志/版本号等外部数据不再依赖手工 escapeHtml(r7 同族风险点)。
import { formatMem, type StatusData } from "./statusStore";

export function StatusCardView({ data }: { data: StatusData }) {
  const degraded = Boolean(data.degraded);
  const notInstalled = !degraded && data.installed === false;
  const running = !degraded && !notInstalled && data.running;
  // degraded(后端不可达)必须如实显示,不得伪装成"内核未安装"
  const stateText = degraded ? "状态不可达" : notInstalled ? "内核未安装" : running ? "运行中" : "已停止";
  const state = degraded ? "degraded" : notInstalled ? "uninstalled" : running ? "running" : "stopped";
  const stats: [string, string][] = [
    ["进程 PID", running ? String(data.pid || "-") : "-"],
    ["CPU 占用", running && typeof data.cpu === "number" ? data.cpu.toFixed(1) + "%" : "-"],
    ["内存占用", running ? formatMem(data.memKB) : "-"],
  ];
  const description = degraded ? "暂时无法获取服务状态，请检查网络或重新登录飞牛后重试。"
    : notInstalled ? "先安装 QwenPaw 内核，即可开始使用 AI 对话。"
    : running ? "服务已就绪，可以进入对话；运行指标会自动更新。"
    : "内核已安装，启动服务后即可继续对话。";
  return (
    <>
      <div class="status-summary">
        <div class="status-head" data-state={state}>
          <span class="status-dot" aria-hidden="true"></span>
          <span class="status-text">{stateText}</span>
        </div>
        <p class="status-description">{description}</p>
      </div>
      <div class="stat-grid" aria-label="运行指标">
        {stats.map(([label, val]) => (
          <div class="stat" key={label}>
            <div class="stat-value">{val}</div>
            <div class="stat-label">{label}</div>
          </div>
        ))}
      </div>
    </>
  );
}

export function LogLines({ lines, empty, success = false }: { lines: (string | { message?: string })[]; empty: string; success?: boolean }) {
  const items = lines.map(item => (typeof item === "string" ? item : (item.message || JSON.stringify(item))));
  if (!items.length) return <span class="log-empty">{empty}</span>;
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

function activateWithKeyboard(event: KeyboardEvent): void {
  if ((event.key === "Enter" || event.key === " ") && event.currentTarget instanceof HTMLElement) {
    event.preventDefault();
    event.currentTarget.click();
  }
}

export function MirrorOptions({ options, value, onPick }: {
  options: { value: string; label: string }[];
  value: string;
  onPick: (v: string) => void;
}) {
  return (
    <>
      {options.map(m => (
        <div class="history-item" key={m.value || "official"} role="button" tabIndex={0}
          aria-pressed={m.value === value} onKeyDown={activateWithKeyboard}
          onClick={() => onPick(m.value)}>
          <span>{m.label}</span>
          {m.value === value ? <span aria-hidden="true">✓</span> : ""}
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
    <div class={`ver-card-item ${active ? "active" : ""}`} role="button" tabIndex={0}
      aria-pressed={active} onKeyDown={activateWithKeyboard} onClick={() => onPick(ver)}>
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
        <div class="history-item" key={item.version} role="button" tabIndex={0}
          onKeyDown={activateWithKeyboard} onClick={() => onPick(item.version)}>
          <span class="history-version">v{item.version}</span>
          <span class={`ver-tag-pill ${item.is_beta ? "beta" : "stable"}`}>{item.is_beta ? "Beta" : "稳定版"}</span>
        </div>
      ))}
    </>
  );
}

export type InstallStep = "downloading" | "installing" | "starting";
export type InstallStepsState = { current: InstallStep | null; outcome: "done" | "failed" | null };

const INSTALL_STEPS: [InstallStep, string][] = [
  ["downloading", "下载依赖"],
  ["installing", "安装内核"],
  ["starting", "启动检查"],
];

export function InstallSteps({ current, outcome }: InstallStepsState) {
  const reached = current ? INSTALL_STEPS.findIndex(([step]) => step === current) : -1;
  return (
    <>
      {INSTALL_STEPS.map(([step, label], index) => {
        const status = outcome === "done" || index < reached ? "done"
          : index === reached ? (outcome === "failed" ? "failed" : "active")
            : "pending";
        return (
          <li key={step} data-step={step} data-status={status} aria-current={status === "active" ? "step" : undefined}>
            <span class="step-mark" aria-hidden="true"></span>
            {label}
          </li>
        );
      })}
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

export function renderInstallSteps(state: InstallStepsState, el: HTMLElement) {
  render(<InstallSteps current={state.current} outcome={state.outcome} />, el);
}
