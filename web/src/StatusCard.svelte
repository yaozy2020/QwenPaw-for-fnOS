<script lang="ts">
  // 打样组件:服务状态卡的 5 行动态信息(服务状态/PID/运行时长/CPU/内存)。
  // 只消费 statusStore,不持有轮询 —— 行为语义(节奏/隐藏页暂停/旁路副作用)
  // 留在 vanilla 的 fetchStatus,本组件纯粹是声明式渲染,便于对照验收。
  import { status, formatUptime, formatMem } from "./statusStore";

  $: notInstalled = $status.installed === false;
  $: running = !notInstalled && $status.running;
  $: stateText = notInstalled ? "内核未安装" : running ? "运行中 (Active)" : "已停止 (Stopped)";
  $: stateColor = notInstalled ? "var(--text-sub)" : running ? "var(--success)" : "var(--danger)";
  // 运行时长随每次快照(5s)重算,与原节奏一致;未运行一律 "-"
  $: uptimeText = running && $status.startAt
    ? formatUptime(Math.floor(Date.now() / 1000) - $status.startAt)
    : "-";
  $: pidText = running ? String($status.pid || "-") : "-";
  $: cpuText = running && typeof $status.cpu === "number" ? $status.cpu.toFixed(1) + "%" : "-";
  $: memText = running ? formatMem($status.memKB) : "-";
</script>

<div class="info-row">
  <span class="info-label">服务状态</span>
  <span class="info-val" style={`color:${stateColor}`}>{stateText}</span>
</div>
<div class="info-row">
  <span class="info-label">进程 PID</span>
  <span class="info-val">{pidText}</span>
</div>
<div class="info-row">
  <span class="info-label">运行时长</span>
  <span class="info-val">{uptimeText}</span>
</div>
<div class="info-row">
  <span class="info-label">CPU 占用</span>
  <span class="info-val">{cpuText}</span>
</div>
<div class="info-row">
  <span class="info-label">内存占用</span>
  <span class="info-val">{memText}</span>
</div>
