import { reactive } from "vue";
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
  const state = reactive({
    busy: false, confirmingRemove: "", canOpen: canOpenDirectory(), entries: null as ProjectDirEntry[] | null,
    listError: "", message: "", error: false,
  });

  function setStage(message: string, error = false): void {
    state.message = message;
    state.error = error;
  }

  function showError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    state.listError = `目录绑定失败：${message}`;
    setStage(state.listError, true);
    showToast(message);
  }

  async function readEntries(): Promise<boolean> {
    try {
      state.entries = await loadProjectDirs();
      state.listError = "";
      if (state.error) setStage("");
      return true;
    } catch (error) {
      state.listError = error instanceof Error ? error.message : String(error);
      setStage(`读取工作区目录失败：${state.listError}`, true);
      return false;
    }
  }

  async function refresh(): Promise<void> {
    if (state.busy) return;
    state.busy = true;
    try { await readEntries(); } finally { state.busy = false; }
  }

  async function add(): Promise<void> {
    if (state.busy) return;
    state.busy = true;
    setStage("正在打开飞牛授权，等待回调…");
    try {
      const result = await authorizeDirectory();
      if (!result.paths.length) throw new Error("授权结果中没有可选目录");
      setStage(`已收到授权目录 ${result.paths[0]}，正在绑定…`);
      await bindAuthorizedDirectory(result.paths[0], stage => {
        setStage(stage === "read" ? "正在读取现有工作区目录…" : "正在保存到 QwenPaw 工作区…");
      });
      const refreshed = await readEntries();
      if (refreshed) {
        setStage("目录已授权并绑定到工作区");
        showToast("目录已授权并绑定到工作区");
      }
    } catch (error) {
      showError(error);
    } finally {
      state.busy = false;
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
    if (state.busy || !canOpenDirectory()) return;
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
    if (state.busy) return;
    state.busy = true;
    try {
      await authorizeDirectory(path);
      const health = await checkDirectory(path);
      await readEntries();
      showToast(health.readable && health.writable ? "目录已重新授权，可读写" : "目录已重新授权，但读写权限仍不足");
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      state.busy = false;
    }
  }

  async function remove(path: string): Promise<void> {
    if (state.busy) return;
    state.busy = true;
    state.confirmingRemove = path;
    try {
      if (!(await confirm({ title: "移除目录绑定", message: `仅从 QwenPaw 工作区移除 ${path}，不会删除目录或文件。`, type: "warn", confirmText: "移除绑定" }))) return;
      state.confirmingRemove = "";
      await unlinkAuthorizedDirectory(path);
      await readEntries();
      showToast("已移除目录绑定，文件未删除");
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      state.confirmingRemove = "";
      state.busy = false;
    }
  }

  async function restorePending(): Promise<void> {
    const result = readStoredAuthorizationResult();
    if (!result?.paths.length || state.busy) return;
    state.busy = true;
    setStage("检测到已完成的授权，正在恢复目录绑定…");
    try {
      await bindAuthorizedDirectory(result.paths[0], stage => {
        setStage(stage === "read" ? "正在读取现有工作区目录…" : "正在保存到 QwenPaw 工作区…");
      });
      if (await readEntries()) {
        clearStoredAuthorizationResult();
        setStage("已恢复授权目录绑定");
      }
    } catch (error) {
      showError(error);
    } finally {
      state.busy = false;
    }
  }

  window.addEventListener("flutterInAppWebViewPlatformReady", () => { state.canOpen = canOpenDirectory(); });
  return { state, refresh, restorePending, add, openDirectory, reauthorize, remove };
}
export type AuthorizedDirsState = ReturnType<typeof createAuthorizedDirsPanel>["state"];
