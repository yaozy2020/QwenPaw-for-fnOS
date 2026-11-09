/** @typedef {"accepted" | "attach" | "adopted"} ClaimSource */
/**
 * @typedef {Object} InstallTask
 * @property {boolean} attach
 * @property {number|null} taskSeq
 * @property {string|null} taskInstance
 * @property {ClaimSource|null} claimedBy
 */
/**
 * @typedef {Object} InstallObservation
 * @property {InstallTask} state
 * @property {boolean} owned
 * @property {boolean} terminal
 * @property {"done" | "failed" | null} outcome
 * @property {"accepted" | "attach" | "adopted" | "seq-mismatch" | "instance-mismatch" | "status-missing-seq" | "status-missing-instance" | "unclaimed"} reason
 */

const TERMINAL_PHASES = new Set(["done", "failed"]);
export const INSTALL_TASK_STORAGE_KEY = "qwenpaw.install-task.v1";

// seq 与 instance 共同构成身份;instance 每次 supervisor 启动不同,跨重启 seq 复用也不会匹配。
/** @param {unknown} value */
function toSeq(value) {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

/** @param {unknown} value */
function toInstance(value) {
  return typeof value === "string" && value.length > 0 ? value : null;
}

/** @param {Record<string, unknown>} status */
function isActive(status) {
  return status.upgrading === true || status.running === true;
}

/**
 * @param {{ attach?: boolean, attachSeq?: number|null, attachInstance?: string|null }} [options]
 * @returns {InstallTask}
 */
export function createInstallTask(options = {}) {
  const attach = Boolean(options.attach);
  const seq = attach ? toSeq(options.attachSeq) : null;
  const instance = attach ? toInstance(options.attachInstance) : null;
  return {
    attach,
    taskSeq: seq,
    taskInstance: instance,
    claimedBy: seq !== null && instance !== null ? "attach" : null,
  };
}

/**
 * 登记受理响应返回的 seq 与 instance;缺任一字段不建立归属。
 * @param {InstallTask} state
 * @param {unknown} seq
 * @param {unknown} instance
 * @returns {InstallTask}
 */
export function acceptInstallTask(state, seq, instance) {
  const acceptedSeq = toSeq(seq);
  const acceptedInstance = toInstance(instance);
  if (acceptedSeq === null || acceptedInstance === null) return state;
  return {
    ...state,
    taskSeq: acceptedSeq,
    taskInstance: acceptedInstance,
    claimedBy: "accepted",
  };
}

/**
 * @param {{ getItem(key: string): string|null, setItem(key: string, value: string): void, removeItem(key: string): void }|null|undefined} storage
 * @param {InstallTask} state
 */
export function persistInstallTask(storage, state) {
  if (!storage || state.taskSeq === null || state.taskInstance === null) return false;
  try {
    storage.setItem(INSTALL_TASK_STORAGE_KEY, JSON.stringify({
      seq: state.taskSeq,
      instance: state.taskInstance,
    }));
    return true;
  } catch {
    return false;
  }
}

/**
 * @param {{ getItem(key: string): string|null }|null|undefined} storage
 * @returns {InstallTask|null}
 */
export function restoreInstallTask(storage) {
  if (!storage) return null;
  try {
    const raw = storage.getItem(INSTALL_TASK_STORAGE_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw);
    if (!saved || typeof saved !== "object") return null;
    const task = createInstallTask({
      attach: true,
      attachSeq: saved.seq,
      attachInstance: saved.instance,
    });
    return task.taskSeq !== null && task.taskInstance !== null ? task : null;
  } catch {
    return null;
  }
}

/**
 * @param {{ removeItem(key: string): void }|null|undefined} storage
 */
export function clearStoredInstallTask(storage) {
  if (!storage) return;
  try {
    storage.removeItem(INSTALL_TASK_STORAGE_KEY);
  } catch {}
}

/**
 * 观察一拍 upgrade_status:每拍 seq 与 instance 都一致,且后端已停止运行,才算终态。
 * @param {InstallTask} state
 * @param {unknown} status
 * @returns {InstallObservation}
 */
export function observeInstallStatus(state, status) {
  const s = status && typeof status === "object"
    ? /** @type {Record<string, unknown>} */ (status)
    : {};
  const seq = toSeq(s.seq);
  const instance = toInstance(s.instance);
  const phase = typeof s.phase === "string" ? s.phase : "";
  const active = isActive(s);

  /** @type {InstallTask} */
  let next = state;
  /** @type {InstallObservation["reason"]} */
  let reason;
  let owned = false;

  if (state.taskSeq !== null && state.taskInstance !== null) {
    owned = seq === state.taskSeq && instance === state.taskInstance;
    if (owned) reason = state.claimedBy || "accepted";
    else if (seq === null) reason = "status-missing-seq";
    else if (instance === null) reason = "status-missing-instance";
    else if (seq !== state.taskSeq) reason = "seq-mismatch";
    else reason = "instance-mismatch";
  } else if (
    state.attach &&
    state.taskSeq === null &&
    active &&
    seq !== null &&
    instance !== null &&
    (state.taskInstance === null || instance === state.taskInstance)
  ) {
    // 附加模式无已选 seq:仅从正在运行且身份齐全的状态绑定一次。
    next = { ...state, taskSeq: seq, taskInstance: instance, claimedBy: "adopted" };
    owned = true;
    reason = "adopted";
  } else {
    if (seq === null) reason = "status-missing-seq";
    else if (instance === null) reason = "status-missing-instance";
    else if (state.taskInstance !== null && instance !== state.taskInstance) reason = "instance-mismatch";
    else reason = "unclaimed";
  }

  const terminal = owned && !active && TERMINAL_PHASES.has(phase);
  return {
    state: next,
    owned,
    terminal,
    outcome: terminal ? /** @type {"done" | "failed"} */ (phase) : null,
    reason,
  };
}
