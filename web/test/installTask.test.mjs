// 安装任务归属:纯逻辑回归。身份 = 受理 seq + supervisor 进程 instance。
import test from "node:test";
import assert from "node:assert/strict";
import {
  createInstallTask,
  acceptInstallTask,
  observeInstallStatus,
  INSTALL_TASK_STORAGE_KEY,
  persistInstallTask,
  restoreInstallTask,
  clearStoredInstallTask,
} from "../src/installTask.mjs";

const INSTANCE = "sup-instance-a";
const OTHER_INSTANCE = "sup-instance-b";

const done = (seq, instance = INSTANCE, extra = {}) => ({ upgrading: false, phase: "done", seq, instance, ...extra });
const failed = (seq, instance = INSTANCE, extra = {}) => ({ upgrading: false, phase: "failed", seq, instance, ...extra });
const running = (seq, instance = INSTANCE, phase = "downloading") => ({ upgrading: true, phase, seq, instance });

const newTask = () => createInstallTask({ attach: false });
const accepted = (seq = 42, instance = INSTANCE) => acceptInstallTask(newTask(), seq, instance);

test("旧 done 不认领:基线未知、未受理身份时后端残留终态不算本次结果", () => {
  const task = newTask();
  const seen = observeInstallStatus(task, done(41));
  assert.equal(seen.owned, false);
  assert.equal(seen.terminal, false);
  assert.equal(seen.outcome, null);
  assert.equal(seen.state.taskSeq, null);
  assert.equal(seen.state.taskInstance, null);
});

test("未受理身份时 idle / failed 残留同样不认领", () => {
  const task = newTask();
  for (const status of [{ upgrading: false, phase: "idle", seq: 41, instance: INSTANCE }, failed(41)]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false, status.phase);
    assert.equal(seen.terminal, false, status.phase);
  }
});

test("已受理身份后首拍即 done 必须正确收口", () => {
  const task = accepted(42);
  assert.equal(task.taskSeq, 42);
  assert.equal(task.taskInstance, INSTANCE);
  assert.equal(task.claimedBy, "accepted");

  const seen = observeInstallStatus(task, done(42));
  assert.equal(seen.owned, true);
  assert.equal(seen.terminal, true);
  assert.equal(seen.outcome, "done");
});

test("已受理身份后首拍即 failed 必须正确收口", () => {
  const task = accepted(42);
  const seen = observeInstallStatus(task, failed(42));
  assert.equal(seen.terminal, true);
  assert.equal(seen.outcome, "failed");
});

test("每拍都要 seq 匹配:已归属 seq 遇到别的 seq 终态不结束", () => {
  const task = accepted(42);
  for (const status of [done(41), done(43), failed(41), running(43)]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false, `seq ${status.seq}/${status.phase}`);
    assert.equal(seen.terminal, false, `seq ${status.seq}/${status.phase}`);
    assert.equal(seen.state.taskSeq, 42);
    assert.equal(seen.state.taskInstance, INSTANCE);
  }
});

test("同 seq 不同 instance 不认领、不结束", () => {
  const task = accepted(42);
  for (const status of [done(42, OTHER_INSTANCE), failed(42, OTHER_INSTANCE), running(42, OTHER_INSTANCE)]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false, status.phase);
    assert.equal(seen.terminal, false, status.phase);
    assert.equal(seen.state.taskSeq, 42);
    assert.equal(seen.state.taskInstance, INSTANCE);
  }
});

test("状态缺少 seq 不能当作本次终态", () => {
  const task = accepted(42);
  const seen = observeInstallStatus(task, { upgrading: false, phase: "done", instance: INSTANCE });
  assert.equal(seen.owned, false);
  assert.equal(seen.terminal, false);
});

test("状态缺少或无效 instance 不能认领", () => {
  const task = accepted(42);
  for (const instance of [undefined, null, "", 42, {}]) {
    const seen = observeInstallStatus(task, { upgrading: false, phase: "done", seq: 42, instance });
    assert.equal(seen.owned, false, String(instance));
    assert.equal(seen.terminal, false, String(instance));
  }
});

test("其他 seq 的运行中任务不会被认领(多设备竞态)", () => {
  const task = newTask();
  const seen = observeInstallStatus(task, running(77));
  assert.equal(seen.owned, false);
  assert.equal(seen.state.taskSeq, null);
  assert.equal(seen.state.taskInstance, null);
});

test("附加模式:已选 seq + instance 匹配时首拍 done 正常收口", () => {
  const task = createInstallTask({ attach: true, attachSeq: 42, attachInstance: INSTANCE });
  assert.equal(task.taskSeq, 42);
  assert.equal(task.taskInstance, INSTANCE);

  const seen = observeInstallStatus(task, done(42));
  assert.equal(seen.owned, true);
  assert.equal(seen.terminal, true);
  assert.equal(seen.outcome, "done");
});

test("附加模式:已选 seq 或 instance 不一致都不结束", () => {
  const task = createInstallTask({ attach: true, attachSeq: 42, attachInstance: INSTANCE });
  for (const status of [done(43), done(42, OTHER_INSTANCE), running(43)]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false, JSON.stringify(status));
    assert.equal(seen.terminal, false, JSON.stringify(status));
  }
});

test("附加模式:未指定 seq 时跟随 running 且身份齐全的任务一次,之后严格匹配", () => {
  const task = createInstallTask({ attach: true });
  const adopted = observeInstallStatus(task, running(50));
  assert.equal(adopted.owned, true);
  assert.equal(adopted.state.taskSeq, 50);
  assert.equal(adopted.state.taskInstance, INSTANCE);
  assert.equal(adopted.terminal, false);

  const doneSeen = observeInstallStatus(adopted.state, done(50));
  assert.equal(doneSeen.terminal, true);
  assert.equal(doneSeen.outcome, "done");

  const otherSeq = observeInstallStatus(adopted.state, done(51));
  assert.equal(otherSeq.owned, false);

  const otherInstance = observeInstallStatus(adopted.state, done(50, OTHER_INSTANCE));
  assert.equal(otherInstance.owned, false);
});

test("附加模式:未指定 seq 但有 instance 提示时,只跟随同 instance 的运行中任务", () => {
  const task = createInstallTask({ attach: true, attachInstance: INSTANCE });

  const wrong = observeInstallStatus(task, running(50, OTHER_INSTANCE));
  assert.equal(wrong.owned, false);
  assert.equal(wrong.state.taskSeq, null);

  const right = observeInstallStatus(task, running(50, INSTANCE));
  assert.equal(right.owned, true);
  assert.equal(right.state.taskSeq, 50);
  assert.equal(right.state.taskInstance, INSTANCE);
});

test("附加模式:未指定 seq 且没有运行中任务时不认领残留终态", () => {
  const task = createInstallTask({ attach: true });
  const seen = observeInstallStatus(task, done(50));
  assert.equal(seen.owned, false);
  assert.equal(seen.terminal, false);
});

test("附加模式:给了 seq 但缺 instance 时不认领,也不改投其他运行中任务", () => {
  const task = createInstallTask({ attach: true, attachSeq: 42 });
  for (const status of [running(42, INSTANCE), running(99, INSTANCE), done(42, INSTANCE)]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false, JSON.stringify(status));
    assert.equal(seen.terminal, false, JSON.stringify(status));
    assert.equal(seen.state.taskInstance, null);
  }
});

test("未知请求保守:空/畸形状态不认领、不结束", () => {
  const task = accepted(42);
  for (const status of [null, undefined, {}, { phase: "done" }, { seq: "42", phase: "done", instance: INSTANCE }]) {
    const seen = observeInstallStatus(task, status);
    assert.equal(seen.owned, false);
    assert.equal(seen.terminal, false);
  }
});

test("受理响应缺少合法 seq 时保持未归属,不猜测", () => {
  for (const seq of [null, undefined, "42", NaN]) {
    const task = acceptInstallTask(newTask(), seq, INSTANCE);
    assert.equal(task.taskSeq, null);
    assert.equal(task.taskInstance, null);
    assert.equal(observeInstallStatus(task, done(42)).terminal, false);
  }
});

test("受理响应缺少或无效 instance 时不认领,也不单独保存 seq", () => {
  for (const instance of [null, undefined, "", 42, {}]) {
    const task = acceptInstallTask(newTask(), 42, instance);
    assert.equal(task.taskSeq, null, String(instance));
    assert.equal(task.taskInstance, null, String(instance));
    assert.equal(observeInstallStatus(task, done(42)).owned, false, String(instance));
  }
});

test("归属只由受理身份或附加选择决定:运行中 phase 变化不会改写身份", () => {
  const task = accepted(42);
  const seen = observeInstallStatus(task, running(42, INSTANCE, "installing"));
  assert.equal(seen.owned, true);
  assert.equal(seen.state.taskSeq, 42);
  assert.equal(seen.state.taskInstance, INSTANCE);

  const other = observeInstallStatus(seen.state, running(43, INSTANCE, "installing"));
  assert.equal(other.owned, false);
  assert.equal(other.state.taskSeq, 42);
  assert.equal(other.state.taskInstance, INSTANCE);
});

test("upgrading 仍为 true 时终态 phase 不收口(等后端停止运行)", () => {
  const task = accepted(42);
  for (const phase of ["done", "failed"]) {
    const upgrading = observeInstallStatus(task, { upgrading: true, phase, seq: 42, instance: INSTANCE });
    assert.equal(upgrading.owned, true, phase);
    assert.equal(upgrading.terminal, false, phase);
    assert.equal(upgrading.outcome, null, phase);

    const stillRunning = observeInstallStatus(task, { upgrading: false, running: true, phase, seq: 42, instance: INSTANCE });
    assert.equal(stillRunning.owned, true, phase);
    assert.equal(stillRunning.terminal, false, phase);
    assert.equal(stillRunning.outcome, null, phase);
  }
});

test("观察不修改传入状态(便于只在确认后提交)", () => {
  const task = createInstallTask({ attach: true, attachInstance: INSTANCE });
  const snapshot = JSON.stringify(task);
  observeInstallStatus(task, running(60));
  assert.equal(JSON.stringify(task), snapshot);
});

function memoryStorage() {
  const values = new Map();
  return {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: key => values.delete(key),
  };
}

test("受理身份写入 sessionStorage 后重建仍恢复同一 seq + instance", () => {
  const storage = memoryStorage();
  const task = accepted(42, INSTANCE);

  assert.equal(persistInstallTask(storage, task), true);
  assert.deepEqual(JSON.parse(storage.getItem(INSTALL_TASK_STORAGE_KEY)), { seq: 42, instance: INSTANCE });

  const restored = restoreInstallTask(storage);
  assert.equal(restored.taskSeq, 42);
  assert.equal(restored.taskInstance, INSTANCE);
  assert.equal(observeInstallStatus(restored, running(42, INSTANCE)).owned, true);
  assert.equal(observeInstallStatus(restored, done(42, INSTANCE)).outcome, "done");
});

test("恢复只认已保存身份:无任务、其他 seq 或 instance 都不被认领", () => {
  const storage = memoryStorage();
  assert.equal(restoreInstallTask(storage), null);
  assert.equal(observeInstallStatus(newTask(), running(42, INSTANCE)).owned, false);

  persistInstallTask(storage, accepted(42, INSTANCE));
  const restored = restoreInstallTask(storage);
  assert.equal(observeInstallStatus(restored, running(43, INSTANCE)).owned, false);
  assert.equal(observeInstallStatus(restored, running(42, OTHER_INSTANCE)).owned, false);
});

test("sessionStorage 不可用时安全降级到本页内存身份", () => {
  const unavailable = {
    getItem() { throw new Error("SecurityError"); },
    setItem() { throw new Error("QuotaExceededError"); },
    removeItem() { throw new Error("SecurityError"); },
  };
  const task = accepted(42, INSTANCE);

  assert.equal(restoreInstallTask(unavailable), null);
  assert.equal(persistInstallTask(unavailable, task), false);
  assert.doesNotThrow(() => clearStoredInstallTask(unavailable));
  assert.equal(observeInstallStatus(task, running(42, INSTANCE)).owned, true);
});
