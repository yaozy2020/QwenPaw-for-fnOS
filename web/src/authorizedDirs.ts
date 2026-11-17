export interface ProjectDirEntry {
  path: string;
  label?: string;
}

export interface AuthResult {
  status: string;
  error: string;
  method: string;
  state: string;
  paths: string[];
}

export function buildAuthorizationUrl(origin: string, state: string, redirectUri: string, path = ""): string {
  const url = new URL(path ? "/app-auth/authorize-user-file" : "/app-auth/pick-user-file", origin);
  url.searchParams.set("appName", "qwenpaw");
  if (path) url.searchParams.set("path", path);
  else url.searchParams.set("directory", "true");
  url.searchParams.set("redirectUri", new URL(redirectUri, origin).toString());
  url.searchParams.set("state", state);
  return url.toString();
}

export function parseAuthCallback(search: string, expectedState: string): AuthResult {
  const params = new URLSearchParams(search);
  const state = params.get("state") || "";
  if (!expectedState || state !== expectedState) throw new Error("授权回调状态不匹配");
  const status = params.get("status") || "";
  const error = params.get("error") || "";
  const method = params.get("method") || "";
  if (status !== "success") throw new Error(error || "已取消授权");
  const raw = params.get("path");
  if (!raw && (method === "authorizeUserFile" || method === "authorizeSharedFile")) {
    return { status, error, method, state, paths: [] };
  }
  if (!raw) throw new Error("授权回调缺少目录");
  let decoded: unknown;
  try { decoded = JSON.parse(raw); } catch { throw new Error("授权目录格式无效"); }
  if (!Array.isArray(decoded)) throw new Error("授权目录格式无效");
  const paths = decoded.filter((path): path is string => typeof path === "string").map(normalizeProjectDir);
  if (!paths.length) throw new Error("授权回调缺少有效目录");
  return { status, error, method, state, paths };
}

type StructuredAuthResult = Partial<Omit<AuthResult, "paths">> & { path?: unknown };

function parseStructuredAuthResult(result: StructuredAuthResult | undefined, expectedState: string, fallbackState = ""): AuthResult {
  const state = result?.state || fallbackState;
  if (!expectedState || state !== expectedState) throw new Error("授权回调状态不匹配");
  const status = result?.status || "";
  const error = result?.error || "";
  const method = result?.method || "";
  if (status !== "success") throw new Error(error || "已取消授权");
  const paths = Array.isArray(result?.path)
    ? result.path.filter((path): path is string => typeof path === "string").map(normalizeProjectDir)
    : [];
  if (!paths.length && method !== "authorizeUserFile" && method !== "authorizeSharedFile") {
    throw new Error("授权回调缺少有效目录");
  }
  return { status, error, method, state, paths };
}

function normalizeProjectDir(path: string): string {
  const trimmed = path.trim();
  if (!trimmed.startsWith("/") || trimmed.includes("\\") || trimmed.includes("\0")) {
    throw new Error("目录必须是绝对路径");
  }
  const normalized = trimmed.replace(/\/+$/, "") || "/";
  if (normalized.split("/").some((part) => part === "..")) throw new Error("目录包含非法路径段");
  return normalized;
}

export function mergeProjectDirs(entries: ProjectDirEntry[], path: string, label = ""): ProjectDirEntry[] {
  const normalized = normalizeProjectDir(path);
  let found = false;
  const merged = entries.map((entry) => {
    if (normalizeProjectDir(entry.path) !== normalized) return entry;
    found = true;
    return { path: normalized, label: label || entry.label };
  });
  if (!found) merged.push({ path: normalized, label });
  return merged;
}

export function removeProjectDir(entries: ProjectDirEntry[], path: string): ProjectDirEntry[] {
  const normalized = normalizeProjectDir(path);
  return entries.filter((entry) => normalizeProjectDir(entry.path) !== normalized);
}

export interface DirectoryHealth {
  path: string;
  readable: boolean;
  writable: boolean;
}

const RESULT_KEY = "qwenpaw.auth-result.v1";
const CALLBACK_PATH = "/app/qwenpaw/auth-callback.html";

function upstreamToken(): string {
  try { return localStorage.getItem("qwenpaw_auth_token") || ""; } catch { return ""; }
}

function managementToken(): string {
  try {
    const part = document.cookie.split(";").find((item) => item.trim().startsWith("__Host-qwenpaw_csrf=") || item.trim().startsWith("qwenpaw_csrf="));
    return part ? part.slice(part.indexOf("=") + 1).trim() : "";
  } catch { return ""; }
}

async function upstreamRequest(path: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  const token = upstreamToken();
  if (token) headers.set("Authorization", `Bearer ${token}`);
  if (init.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  return fetch(`/app/qwenpaw/web/api${path}`, { ...init, headers });
}

function upstreamError(data: { detail?: unknown; message?: unknown }, fallback: string): Error {
  const detail = data.detail ?? data.message;
  if (typeof detail === "string") return new Error(detail);
  return new Error(detail == null ? fallback : JSON.stringify(detail));
}

export async function loadProjectDirs(): Promise<ProjectDirEntry[]> {
  const response = await upstreamRequest("/workspace/project-directory/dirs");
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw upstreamError(data, `读取工作区目录失败 (${response.status})`);
  return Array.isArray(data.project_dirs) ? data.project_dirs : [];
}

async function saveProjectDirs(entries: ProjectDirEntry[]): Promise<void> {
  const response = await upstreamRequest("/workspace/project-directory/dirs", {
    method: entries.length ? "PUT" : "DELETE",
    body: entries.length ? JSON.stringify({
      project_dirs: entries.map(entry => ({
        path: normalizeProjectDir(entry.path),
        ...(entry.label?.trim() ? { label: entry.label.trim() } : {}),
      })),
    }) : undefined,
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw upstreamError(data, `保存工作区目录失败 (${response.status})`);
}

export async function checkDirectory(path: string): Promise<DirectoryHealth> {
  const response = await fetch("/app/qwenpaw/api/authorized-dirs/check", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": managementToken() },
    body: JSON.stringify({ path }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok || !data.success) throw new Error(data.message || data.error || `目录权限复查失败 (${response.status})`);
  return data as DirectoryHealth;
}

export async function bindAuthorizedDirectory(path: string, onStage?: (stage: "read" | "save") => void): Promise<void> {
  onStage?.("read");
  const entries = await loadProjectDirs();
  onStage?.("save");
  await saveProjectDirs(mergeProjectDirs(entries, path));
}

export async function unlinkAuthorizedDirectory(path: string): Promise<void> {
  const entries = await loadProjectDirs();
  await saveProjectDirs(removeProjectDir(entries, path));
}

function randomState(): string {
  try { return crypto.randomUUID(); } catch { return `${Date.now()}-${Math.random().toString(16).slice(2)}`; }
}

interface AuthSurface {
  closed: boolean;
  navigate(url: string): void;
  close(): void;
}

function openAuthorizationSurface(): AuthSurface {
  if (window.parent !== window) {
    const overlay = document.createElement("div");
    overlay.style.cssText = "position:fixed;inset:0;z-index:2147483000;display:flex;flex-direction:column;background:var(--bg,#f3f3f0)";
    const bar = document.createElement("div");
    bar.style.cssText = "display:flex;justify-content:space-between;align-items:center;padding:12px 16px;border-bottom:1px solid var(--card-border,#ded6cb)";
    const title = document.createElement("strong");
    title.textContent = "飞牛目录授权";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "取消";
    cancel.style.cssText = "min-height:36px;padding:4px 12px";
    const frame = document.createElement("iframe");
    frame.title = "飞牛目录授权";
    frame.style.cssText = "flex:1;width:100%;border:0;background:#fff";
    let closed = false;
    const close = () => {
      if (closed) return;
      closed = true;
      overlay.remove();
    };
    cancel.addEventListener("click", close);
    overlay.addEventListener("keydown", (event) => { if (event.key === "Escape") close(); });
    bar.append(title, cancel);
    overlay.append(bar, frame);
    document.body.appendChild(overlay);
    return { get closed() { return closed; }, navigate: (next) => { frame.src = next; }, close };
  }
  const popup = window.open("about:blank", "_blank", "width=720,height=760");
  if (!popup) throw new Error("浏览器拦截了授权窗口，请允许弹窗后重试");
  return {
    get closed() { return popup.closed; },
    navigate: (next) => popup.location.replace(next),
    close: () => popup.close(),
  };
}

function waitForAuthorization(path: string): Promise<AuthResult> {
  const state = randomState();
  const url = buildAuthorizationUrl(window.location.origin, state, CALLBACK_PATH, path);
  const surface = openAuthorizationSurface();
  return new Promise((resolve, reject) => {
    let settled = false;
    let resultPolling = false;
    let closing = false;
    const finish = (error?: Error, result?: AuthResult) => {
      if (settled) return;
      settled = true;
      window.removeEventListener("message", onMessage);
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("focus", lookupStored);
      document.removeEventListener("visibilitychange", onVisibility);
      clearInterval(pollTimer);
      clearInterval(resultPollTimer);
      clearTimeout(timeout);
      try { localStorage.removeItem(RESULT_KEY); } catch {}
      surface.close();
      if (error) reject(error);
      else if (result) resolve(result);
    };
    const accept = (payload: { state?: string; result?: StructuredAuthResult; search?: string }) => {
      if (!payload || (payload.state && payload.state !== state) || (payload.result?.state && payload.result.state !== state) || (payload.state === undefined && payload.result?.state === undefined && !payload.search?.includes(`state=${state}`))) return;
      try {
        const result = payload.search
          ? parseAuthCallback(payload.search, state)
          : parseStructuredAuthResult(payload.result, state, payload.state);
        finish(undefined, result);
      } catch (error) { finish(error as Error); }
    };
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== window.location.origin) return;
      accept(event.data || {});
    };
    const onStorage = (event: StorageEvent) => {
      if (event.key !== RESULT_KEY || !event.newValue) return;
      try { accept(JSON.parse(event.newValue)); } catch {}
    };
    const lookupStored = () => {
      try {
        const raw = localStorage.getItem(RESULT_KEY);
        if (raw) accept(JSON.parse(raw));
      } catch {}
    };
    const onVisibility = () => {
      if (!document.hidden) lookupStored();
    };
    const pollResult = async () => {
      if (settled || resultPolling) return;
      resultPolling = true;
      try {
        const search = await readAuthorizationResult(state);
        if (search) accept({ state, search });
      } catch {} finally {
        resultPolling = false;
      }
    };
    const pollTimer = window.setInterval(() => {
      if (!surface.closed || closing) return;
      closing = true;
      void pollResult().finally(() => {
        if (!settled) finish(new Error("已取消授权"));
      });
    }, 500);
    const resultPollTimer = window.setInterval(() => void pollResult(), 900);
    const timeout = window.setTimeout(() => finish(new Error("授权超时，请重试")), 5 * 60 * 1000);
    window.addEventListener("message", onMessage);
    window.addEventListener("storage", onStorage);
    window.addEventListener("focus", lookupStored);
    document.addEventListener("visibilitychange", onVisibility);
    lookupStored();
    if (!settled) surface.navigate(url);
    void pollResult();
  });
}

export function readStoredAuthorizationResult(now = Date.now()): AuthResult | null {
  try {
    const raw = localStorage.getItem(RESULT_KEY);
    if (!raw) return null;
    const stored = JSON.parse(raw) as { ts?: number; state?: string; result?: StructuredAuthResult; search?: string };
    if (typeof stored.state !== "string" || !stored.state) return null;
    if (typeof stored.ts !== "number" || !Number.isFinite(stored.ts) || Math.abs(now - stored.ts) > 5 * 60 * 1000) {
      localStorage.removeItem(RESULT_KEY);
      return null;
    }
    return stored.search
      ? parseAuthCallback(stored.search, stored.state)
      : parseStructuredAuthResult(stored.result, stored.state, stored.state);
  } catch {
    return null;
  }
}

export function clearStoredAuthorizationResult(): void {
  try { localStorage.removeItem(RESULT_KEY); } catch {}
}

export async function readAuthorizationResult(state: string): Promise<string> {
  const response = await fetch(`/app/qwenpaw/api/authorized-dirs/result?state=${encodeURIComponent(state)}`, { cache: "no-store" });
  if (!response.ok) return "";
  const data = await response.json().catch(() => ({}));
  return data.ok && typeof data.search === "string" ? data.search : "";
}

export async function authorizeDirectory(path = ""): Promise<AuthResult> {
  return waitForAuthorization(path);
}
