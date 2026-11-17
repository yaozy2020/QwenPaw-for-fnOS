import { TrimApp } from "@trimjs/web-app";
import {
  authorizeDirectory,
  bindAuthorizedDirectory,
  checkDirectory,
  clearStoredAuthorizationResult,
  loadProjectDirs,
  readStoredAuthorizationResult,
  unlinkAuthorizedDirectory,
  type ProjectDirEntry,
} from "./authorizedDirs";

interface AuthorizedDirsPanelOptions {
  showToast(message: string): void;
  confirm(options: { title: string; message: string; type: "warn" | "primary" | "danger"; confirmText: string }): Promise<boolean>;
}

export function createAuthorizedDirsPanel({ showToast, confirm }: AuthorizedDirsPanelOptions) {
  const sdk = new TrimApp();
  const canOpenDirectory = () => window.parent !== window || !sdk.isWeb;
  let busy = false;
  let renderedEntries: ProjectDirEntry[] = [];

  function render(entries: ProjectDirEntry[]): void {
    renderedEntries = entries;
    const list = document.getElementById("authorizedDirsList");
    if (!list) return;
    list.replaceChildren();
    if (!entries.length) {
      const empty = document.createElement("span");
      empty.className = "log-empty";
      empty.textContent = "尚未绑定项目目录。选择飞牛目录后，可在对话和文件区中使用。";
      list.append(empty);
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("div");
      row.className = "info-row";
      const path = document.createElement("span");
      path.className = "authorized-dir-value";
      path.textContent = entry.label ? `${entry.label} · ${entry.path}` : entry.path;
      path.title = entry.path;
      const actions = document.createElement("div");
      actions.className = "authorized-dir-actions";
      if (canOpenDirectory()) {
        const open = document.createElement("button");
        open.type = "button";
        open.className = "btn-outline btn-xs";
        open.textContent = "打开目录";
        open.addEventListener("click", () => void openDirectory(entry.path));
        actions.append(open);
      }
      const reauth = document.createElement("button");
      reauth.type = "button";
      reauth.className = "btn-outline btn-xs";
      reauth.textContent = "重新授权";
      reauth.addEventListener("click", () => void reauthorize(entry.path));
      const removeButton = document.createElement("button");
      removeButton.type = "button";
      removeButton.className = "btn-danger-ghost btn-xs";
      removeButton.textContent = "移除绑定";
      removeButton.addEventListener("click", () => void remove(entry.path));
      actions.append(reauth, removeButton);
      row.append(path, actions);
      list.append(row);
    }
  }

  function setBusy(value: boolean): void {
    busy = value;
    for (const id of ["addAuthorizedDirBtn", "refreshAuthorizedDirsBtn"]) {
      const button = document.getElementById(id) as HTMLButtonElement | null;
      if (button) button.disabled = value;
    }
  }

  function setStage(message: string, error = false): void {
    const status = document.getElementById("authorizedDirsStatus");
    if (!status) return;
    status.textContent = message;
    status.dataset.error = String(error);
  }

  function showError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    const list = document.getElementById("authorizedDirsList");
    if (list) list.textContent = `目录绑定失败：${message}`;
    setStage(`目录绑定失败：${message}`, true);
    showToast(message);
  }

  async function refresh(): Promise<boolean> {
    try {
      render(await loadProjectDirs());
      return true;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const list = document.getElementById("authorizedDirsList");
      if (list) list.textContent = message;
      setStage(`读取工作区目录失败：${message}`, true);
      return false;
    }
  }

  async function add(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setStage("正在打开飞牛授权，等待回调…");
    try {
      const result = await authorizeDirectory();
      if (!result.paths.length) throw new Error("授权结果中没有可选目录");
      setStage(`已收到授权目录 ${result.paths[0]}，正在绑定…`);
      await bindAuthorizedDirectory(result.paths[0], stage => {
        setStage(stage === "read" ? "正在读取现有工作区目录…" : "正在保存到 QwenPaw 工作区…");
      });
      const refreshed = await refresh();
      if (refreshed) {
        setStage("目录已授权并绑定到工作区");
        showToast("目录已授权并绑定到工作区");
      }
    } catch (error) {
      showError(error);
    } finally {
      setBusy(false);
    }
  }

  async function mobileAppMessageDiagnostics(): Promise<string> {
    try {
      const result: unknown = await sdk.callAppMethod("getAppMessage");
      if (result == null) return `getAppMessage=${String(result)}`;
      if (typeof result !== "object") return `getAppMessage类型=${typeof result}`;
      const record = result as Record<string, unknown>;
      const keys = Object.keys(record).sort().join(",");
      const appApi = typeof record.appApi === "string" ? record.appApi : "缺失";
      return `getAppMessage字段=${keys || "无"}，appApi=${appApi}`;
    } catch (error) {
      return `getAppMessage读取失败=${error instanceof Error ? error.message : String(error)}`;
    }
  }

  async function openDirectory(path: string): Promise<void> {
    if (!canOpenDirectory()) return;
    try {
      await sdk.ready();
      const result: unknown = await sdk.openFileManager(path);
      const record = result && typeof result === "object" ? result as Record<string, unknown> : null;
      const failed = result === false
        || record?.success === false
        || record?.ok === false
        || (typeof record?.code === "number" && record.code !== 0);
      const detail = result === undefined ? "无返回值" : typeof result === "string" ? result : JSON.stringify(result);
      if (failed) throw new Error((typeof record?.message === "string" && record.message) || detail);
      setStage(`openFileManager 返回：${detail || "空结果"}；请确认文件管理器是否打开`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const diagnostics = sdk.isWeb ? "" : `；${await mobileAppMessageDiagnostics()}`;
      setStage(`打开目录失败：${message}${diagnostics}`, true);
      showToast(`打开目录失败：${message}${diagnostics}`);
    }
  }

  async function reauthorize(path: string): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      await authorizeDirectory(path);
      const health = await checkDirectory(path);
      await refresh();
      showToast(health.readable && health.writable ? "目录已重新授权，可读写" : "目录已重新授权，但读写权限仍不足");
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function remove(path: string): Promise<void> {
    if (busy) return;
    if (!(await confirm({ title: "移除目录绑定", message: `仅从 QwenPaw 工作区移除 ${path}，不会删除目录或文件。`, type: "warn", confirmText: "移除绑定" }))) return;
    setBusy(true);
    try {
      await unlinkAuthorizedDirectory(path);
      await refresh();
      showToast("已移除目录绑定，文件未删除");
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function restorePending(): Promise<void> {
    const result = readStoredAuthorizationResult();
    if (!result?.paths.length || busy) return;
    setBusy(true);
    setStage("检测到已完成的授权，正在恢复目录绑定…");
    try {
      await bindAuthorizedDirectory(result.paths[0], stage => {
        setStage(stage === "read" ? "正在读取现有工作区目录…" : "正在保存到 QwenPaw 工作区…");
      });
      if (await refresh()) {
        clearStoredAuthorizationResult();
        setStage("已恢复授权目录绑定");
      }
    } catch (error) {
      showError(error);
    } finally {
      setBusy(false);
    }
  }

  document.getElementById("addAuthorizedDirBtn")?.addEventListener("click", () => void add());
  document.getElementById("refreshAuthorizedDirsBtn")?.addEventListener("click", () => void refresh());
  window.addEventListener("flutterInAppWebViewPlatformReady", () => render(renderedEntries));

  return { refresh, restorePending, add, openDirectory, reauthorize, remove };
}
