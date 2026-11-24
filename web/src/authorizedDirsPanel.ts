import { reactive } from "vue";
import { TrimApp } from "@trimjs/web-app";
import { currentLocale, t } from "./i18n";
import {
  authorizeDirectory,
  bindAuthorizedDirectory,
  checkDirectory,
  clearStoredAuthorizationResult,
  loadProjectDirs,
  readStoredAuthorizationResult,
  unlinkAuthorizedDirectory,
  type ProjectDirEntry,
  type AuthSurface,
} from "./authorizedDirs";

interface AuthorizedDirsPanelOptions {
  showToast(message: string): void;
  confirm(options: { title: string; message: string; type: "warn" | "primary" | "danger"; confirmText: string }): Promise<boolean>;
  sdk?: TrimApp;
}

const emptyText = () => "";

export function createAuthorizedDirsPanel({ showToast, confirm, sdk = new TrimApp() }: AuthorizedDirsPanelOptions) {
  const canOpenDirectory = () => window.parent !== window || !sdk.isWeb;
  const raw = reactive({
    busy: false, confirmingRemove: "", canOpen: canOpenDirectory(), entries: null as ProjectDirEntry[] | null,
    listErrorText: emptyText as () => string, messageText: emptyText as () => string, error: false,
    surfaceOpen: false, surfaceUrl: "about:blank", surfaceTrigger: null as HTMLElement | null,
  });
  const authorizationSurface: AuthSurface = {
    get closed() { return !raw.surfaceOpen; },
    navigate(url) { raw.surfaceUrl = url; raw.surfaceOpen = true; },
    close() { raw.surfaceOpen = false; raw.surfaceUrl = "about:blank"; },
  };
  const surfaceForWindow = () => window.parent !== window ? authorizationSurface : undefined;
  const state = {
    get busy() { return raw.busy; },
    get confirmingRemove() { return raw.confirmingRemove; },
    get canOpen() { return raw.canOpen; },
    get entries() { return raw.entries; },
    get listError() { return raw.listErrorText(); },
    get message() { return raw.messageText(); },
    get error() { return raw.error; },
    surface: {
      get open() { return raw.surfaceOpen; },
      get url() { return raw.surfaceUrl; },
      get trigger() { return raw.surfaceTrigger; },
      close: authorizationSurface.close,
    },
  };

  function setStage(message: () => string, error = false): void {
    raw.messageText = message;
    raw.error = error;
  }

  function showError(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    const formatted = () => t("authz.bindFailed", { error: message });
    raw.listErrorText = formatted;
    setStage(formatted, true);
    showToast(message);
  }

  async function readEntries(): Promise<boolean> {
    try {
      raw.entries = await loadProjectDirs();
      raw.listErrorText = emptyText;
      if (raw.error) setStage(emptyText);
      return true;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      raw.listErrorText = () => detail;
      setStage(() => t("authz.readFailed", { error: detail }), true);
      return false;
    }
  }

  async function refresh(): Promise<void> {
    if (raw.busy) return;
    raw.busy = true;
    try { await readEntries(); } finally { raw.busy = false; }
  }

  async function add(): Promise<void> {
    if (raw.busy) return;
    raw.surfaceTrigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    raw.busy = true;
    setStage(() => t("authz.opening"));
    try {
      const result = await authorizeDirectory("", surfaceForWindow());
      if (!result.paths.length) throw new Error(t("authz.noSelectableDir"));
      const firstPath = result.paths[0];
      setStage(() => t("authz.receivedBinding", { path: firstPath }));
      await bindAuthorizedDirectory(firstPath, stage => {
        setStage(() => (stage === "read" ? t("authz.readingExisting") : t("authz.savingWorkspace")));
      });
      const refreshed = await readEntries();
      if (refreshed) {
        setStage(() => t("authz.bindSuccess"));
        showToast(t("authz.bindSuccess"));
      }
    } catch (error) {
      showError(error);
    } finally {
      raw.busy = false;
    }
  }

  async function mobileAppMessageDiagnostics(): Promise<string> {
    try {
      const result: unknown = await sdk.callAppMethod("getAppMessage");
      if (result == null) return `getAppMessage=${String(result)}`;
      if (typeof result !== "object") return t("authz.diag.type", { type: typeof result });
      const record = result as Record<string, unknown>;
      const keys = Object.keys(record).sort().join(",");
      const appApi = typeof record.appApi === "string" ? record.appApi : t("authz.diag.missing");
      return t("authz.diag.fields", { keys: keys || t("authz.diag.none"), appApi });
    } catch (error) {
      return t("authz.diag.readFailed", { error: error instanceof Error ? error.message : String(error) });
    }
  }

  async function openDirectory(path: string): Promise<void> {
    if (raw.busy || !canOpenDirectory()) return;
    try {
      await sdk.ready();
      const result: unknown = await sdk.openFileManager(path);
      const record = result && typeof result === "object" ? result as Record<string, unknown> : null;
      const failed = result === false
        || record?.success === false
        || record?.ok === false
        || (typeof record?.code === "number" && record.code !== 0);
      const detail = result === undefined ? "" : typeof result === "string" ? result : JSON.stringify(result);
      if (failed) throw new Error((typeof record?.message === "string" && record.message) || detail || t("authz.noReturn"));
      setStage(() => t("authz.openReturned", { detail: result === undefined ? t("authz.noReturn") : (detail || t("authz.emptyResult")) }));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const diagText = sdk.isWeb ? "" : await mobileAppMessageDiagnostics();
      const formatFailed = () => {
        const sep = currentLocale.value === "en" ? "; " : "；";
        const diagnostics = diagText ? `${sep}${diagText}` : "";
        return t("authz.openFailed", { error: message, diagnostics });
      };
      setStage(formatFailed, true);
      showToast(formatFailed());
    }
  }

  async function reauthorize(path: string): Promise<void> {
    if (raw.busy) return;
    raw.surfaceTrigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    raw.busy = true;
    try {
      await authorizeDirectory(path, surfaceForWindow());
      const health = await checkDirectory(path);
      await readEntries();
      showToast(health.readable && health.writable ? t("authz.reauthOk") : t("authz.reauthLimited"));
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      raw.busy = false;
    }
  }

  async function remove(path: string): Promise<void> {
    if (raw.busy) return;
    raw.busy = true;
    raw.confirmingRemove = path;
    try {
      if (!(await confirm({
        title: t("authz.removeTitle"),
        message: t("authz.removeMsg", { path }),
        type: "warn",
        confirmText: t("authz.removeConfirm"),
      }))) return;
      raw.confirmingRemove = "";
      await unlinkAuthorizedDirectory(path);
      await readEntries();
      showToast(t("authz.removedToast"));
    } catch (error) {
      showToast(error instanceof Error ? error.message : String(error));
    } finally {
      raw.confirmingRemove = "";
      raw.busy = false;
    }
  }

  async function restorePending(): Promise<void> {
    const result = readStoredAuthorizationResult();
    if (!result?.paths.length || raw.busy) return;
    raw.busy = true;
    setStage(() => t("authz.restoring"));
    try {
      await bindAuthorizedDirectory(result.paths[0], stage => {
        setStage(() => (stage === "read" ? t("authz.readingExisting") : t("authz.savingWorkspace")));
      });
      if (await readEntries()) {
        clearStoredAuthorizationResult();
        setStage(() => t("authz.restored"));
      }
    } catch (error) {
      showError(error);
    } finally {
      raw.busy = false;
    }
  }

  const onPlatformReady = () => { raw.canOpen = canOpenDirectory(); };
  window.addEventListener("flutterInAppWebViewPlatformReady", onPlatformReady);
  return {
    state, refresh, restorePending, add, openDirectory, reauthorize, remove,
    destroy: () => {
      window.removeEventListener("flutterInAppWebViewPlatformReady", onPlatformReady);
      authorizationSurface.close();
      raw.surfaceTrigger = null;
    },
  };
}
export type AuthorizedDirsState = ReturnType<typeof createAuthorizedDirsPanel>["state"];
