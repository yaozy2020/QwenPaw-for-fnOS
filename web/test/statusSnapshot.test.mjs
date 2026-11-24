import test from "node:test";
import assert from "node:assert/strict";
import { formatUptime, isStatusSnapshot } from "../src/statusStore.ts";
import { createStatusPanel } from "../src/statusPanel.ts";
import { createManagementState } from "../src/managementState.ts";

test('uptime folds seconds at 24 hours and truncates to the last completed minute', () => {
  for (const [seconds, expected] of [[59, '59秒'], [60, '1分 0秒'], [3600, '01:00:00'],
    [86399, '23:59:59'], [86400, '1天 00:00'], [86459, '1天 00:00'], [86460, '1天 00:01'],
    [90065, '1天 01:01'], [172799, '1天 23:59'], [172800, '2天 00:00']]) {
    assert.equal(formatUptime(seconds), expected);
  }
});

const snapshot = {
  success: true, installed: true, running: true, pid: "421", startAt: 1794900000,
  version: "2.3.0", adopted: false, authEnabled: true, authStatus: "protected",
  port: 18088, cpu: 1.5, memKB: 102400,
  install: {
    phase: "installing", fetched: 0, message: "", error: "", seq: 42, instance: "worker-a",
    running: true, recovery_required: false, recovery_error: "",
  },
};

test("complete Rust status snapshots accept running, stopped and uninstalled states", () => {
  assert.equal(isStatusSnapshot(snapshot), true);
  assert.equal(isStatusSnapshot({ ...snapshot, pid: 421 }), true);
  assert.equal(isStatusSnapshot({ ...snapshot, cpu: null }), true, 'first CPU delta is unknown');
  assert.equal(isStatusSnapshot({
    ...snapshot, installed: false, running: false, pid: "0", startAt: 0, version: "",
    authEnabled: false, authStatus: "unknown", cpu: 0, memKB: 0,
    install: { running: false },
  }), true);
  assert.deepEqual(snapshot.install, {
    phase: "installing", fetched: 0, message: "", error: "", seq: 42, instance: "worker-a",
    running: true, recovery_required: false, recovery_error: "",
  }, "validation must leave installation task identity untouched");
});

test("resource metrics do not replace the process snapshot object", async () => {
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  globalThis.window = { setInterval, clearInterval };
  globalThis.document = { hidden: false };
  try {
    const panel = createStatusPanel({
      api: { status: async () => ({ ok: true, status: 200, data: snapshot }) },
      state: createManagementState(),
      versionPanel: { setCurrentRunningVersion() {} },
      errorMessage: String,
      updateOverviewInstaller() {},
      visible: () => true,
    });
    await panel.refresh();
    const before = panel.view.data;
    panel.consumeMetrics(JSON.stringify({ pid: 421, startAt: snapshot.startAt, cpu: 12.5, memKB: 2048 }));
    assert.equal(panel.view.data, before);
    assert.equal(panel.view.cpu, 12.5);
    assert.equal(panel.view.memKB, 2048);
    panel.destroy();
  } finally {
    globalThis.window = originalWindow;
    globalThis.document = originalDocument;
  }
});

test("partial, failed, malformed and wrongly typed event data cannot become service state", () => {
  for (const value of [null, [], "snapshot", {}, { ok: false }, { success: false },
    { success: true, installed: true, running: false }]) {
    assert.equal(isStatusSnapshot(value), false, JSON.stringify(value));
  }
  for (const field of ["success", "installed", "running", "pid", "startAt", "version", "port", "cpu", "memKB", "authEnabled", "authStatus", "install"]) {
    const partial = { ...snapshot };
    delete partial[field];
    assert.equal(isStatusSnapshot(partial), false, `missing ${field}`);
  }
  for (const patch of [
    { success: false }, { installed: "true" }, { running: "false" },
    { pid: "<script>" }, { pid: -1 }, { pid: 0.5 }, { pid: null },
    { startAt: -1 }, { startAt: Infinity }, { startAt: "1794900000" },
    { cpu: NaN }, { cpu: "1.5" }, { cpu: -1 }, { memKB: -1 },
    { version: {} }, { port: "18088" }, { port: 0 }, { port: 65536 },
    { authEnabled: "false" }, { authStatus: "unexpected" },
    { install: null }, { install: {} }, { install: { running: "false" } },
  ]) {
    assert.equal(isStatusSnapshot({ ...snapshot, ...patch }), false, JSON.stringify(patch));
  }
});
