export interface ApiMessageResponse {
  success?: boolean;
  message?: string;
}

export interface VersionItem {
  version: string;
  is_beta: boolean;
  action?: string;
}

export interface StatusResponse extends ApiMessageResponse {
  installed?: boolean;
  running?: boolean;
  pid?: number | string | null;
  startAt?: number | null;
  cpu?: number | null;
  memKB?: number | null;
  install?: { running?: boolean };
  version?: string;
  authStatus?: string;
  authEnabled?: boolean;
  port?: number | string;
}

export interface VersionListResponse extends ApiMessageResponse {
  versions?: VersionItem[];
  latest_stable?: string;
  latest_beta?: string;
}

export interface MirrorResponse extends ApiMessageResponse {
  mirror?: string;
}

export interface InstallStatusResponse extends ApiMessageResponse {
  upgrading?: boolean;
  running?: boolean;
  seq?: number;
  instance?: string;
  phase?: string;
  error?: string;
  recovery_error?: string;
  recovery_required?: boolean;
}

export type LogEntry = string | { message?: string };

export interface LogsResponse extends ApiMessageResponse {
  logs?: LogEntry[];
}

export interface ApiResult<T> {
  status: number;
  ok: boolean;
  data: T;
}

export type ApiClientErrorKind = "network" | "non-json" | "json";

export class ApiClientError extends Error {
  readonly kind: ApiClientErrorKind;
  readonly status: number | null;

  constructor(kind: ApiClientErrorKind, message: string, status: number | null = null) {
    super(message);
    this.name = "ApiClientError";
    this.kind = kind;
    this.status = status;
  }
}

export type FetchLike = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export interface ApiClientOptions {
  pathname: string;
  cookie?: () => string;
  fetch?: FetchLike;
  now?: () => number;
  readTimeoutMs?: number;
}

export type ServiceAction = "start" | "stop" | "restart";

function isGatewayPathname(pathname: string): boolean {
  return pathname === "/app/qwenpaw" || pathname.startsWith("/app/qwenpaw/");
}

export function deriveApiBase(pathname: string): string {
  if (!isGatewayPathname(pathname)) {
    throw new Error("Gateway API requires the /app/qwenpaw console path");
  }
  return "/app/qwenpaw/api";
}

function cookieValue(cookie: string, name: string): string | undefined {
  let value: string | undefined;
  for (const part of cookie.split(";")) {
    const separator = part.indexOf("=");
    if (separator < 0 || part.slice(0, separator).trim() !== name) continue;
    if (value !== undefined) return "";
    value = part.slice(separator + 1).trim();
  }
  return value;
}

export function readCsrfToken(cookie: string): string {
  return cookieValue(cookie, "__Host-qwenpaw_csrf")
    ?? cookieValue(cookie, "qwenpaw_csrf")
    ?? "";
}

function isJsonContentType(contentType: string): boolean {
  const mediaType = contentType.split(";", 1)[0]?.trim().toLowerCase() || "";
  return mediaType === "application/json" || (mediaType.startsWith("application/") && mediaType.endsWith("+json"));
}

export interface ApiClient {
  readonly base: string;
  status(): Promise<ApiResult<StatusResponse>>;
  changePort(port: number): Promise<ApiResult<ApiMessageResponse>>;
  webProxyPort(): Promise<ApiResult<{ port: number; public: boolean }>>;
  changeWebProxyPort(port: number, publicAccess: boolean): Promise<ApiResult<ApiMessageResponse & { port: number }>>;
  resetAuth(): Promise<ApiResult<ApiMessageResponse>>;
  uninstallKernel(): Promise<ApiResult<ApiMessageResponse>>;
  controlService(action: ServiceAction): Promise<ApiResult<ApiMessageResponse>>;
  getMirror(): Promise<ApiResult<MirrorResponse>>;
  setMirror(mirror: string): Promise<ApiResult<ApiMessageResponse>>;
  versionList(): Promise<ApiResult<VersionListResponse>>;
  upgradeStatus(): Promise<ApiResult<InstallStatusResponse>>;
  upgrade(version: string, mirror: string): Promise<ApiResult<InstallStatusResponse>>;
  upgradeLogs(seq: number, instance: string): Promise<ApiResult<LogsResponse>>;
  logs(lines: number): Promise<ApiResult<LogsResponse>>;
  clearLogs(): Promise<ApiResult<ApiMessageResponse>>;
}

export function createApiClient(options: ApiClientOptions): ApiClient {
  const base = deriveApiBase(options.pathname);
  const cookie = options.cookie || (() => typeof document === "undefined" ? "" : document.cookie);
  const fetcher = options.fetch || globalThis.fetch.bind(globalThis);
  const now = options.now || Date.now;
  const configuredReadTimeout = options.readTimeoutMs;
  const readTimeoutMs = typeof configuredReadTimeout === "number"
    && Number.isFinite(configuredReadTimeout)
    && configuredReadTimeout > 0
    ? configuredReadTimeout
    : 5_000;

  function csrfHeaders(): Record<string, string> {
    const token = readCsrfToken(cookie());
    return token ? { "X-CSRF-Token": token } : {};
  }

  async function request<T>(url: string, init?: RequestInit, timeoutMs?: number): Promise<ApiResult<T>> {
    let controller: AbortController | undefined;
    if (timeoutMs !== undefined && typeof globalThis.AbortController === "function") {
      try {
        controller = new globalThis.AbortController();
      } catch {}
    }
    const requestInit = controller ? { ...init, signal: controller.signal } : init;
    const operation = async (): Promise<ApiResult<T>> => {
      let response: Response;
      try {
        response = await fetcher(url, requestInit);
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new ApiClientError("network", `API network request failed: ${detail}`);
      }

      if (!isJsonContentType(response.headers.get("content-type") || "")) {
        throw new ApiClientError("non-json", `API returned a non-JSON response (HTTP ${response.status})`, response.status);
      }

      let data: T;
      try {
        data = await response.json() as T;
      } catch (error) {
        const detail = error instanceof Error ? `: ${error.message}` : "";
        throw new ApiClientError("json", `API returned invalid or truncated JSON (HTTP ${response.status})${detail}`, response.status);
      }
      return { status: response.status, ok: response.ok, data };
    };

    if (timeoutMs === undefined) return operation();

    let timeout: ReturnType<typeof setTimeout> | undefined;
    const timedOut = new Promise<never>((_resolve, reject) => {
      timeout = setTimeout(() => {
        try { controller?.abort(); } catch {}
        reject(new ApiClientError("network", `API request timed out after ${timeoutMs}ms`));
      }, timeoutMs);
    });
    try {
      return await Promise.race([operation(), timedOut]);
    } finally {
      if (timeout !== undefined) clearTimeout(timeout);
    }
  }

  function get<T>(route: string, routeParams = ""): Promise<ApiResult<T>> {
    const query = routeParams ? `${routeParams}&_t=${now()}` : `_t=${now()}`;
    return request<T>(`${base}/${route}?${query}`, undefined, readTimeoutMs);
  }

  function post<T>(route: string, body?: unknown, keepalive = false): Promise<ApiResult<T>> {
    const headers = new Headers(csrfHeaders());
    const init: RequestInit = { method: "POST", headers, keepalive };
    if (body !== undefined) {
      headers.set("Content-Type", "application/json");
      init.body = JSON.stringify(body);
    }
    return request<T>(`${base}/${route}`, init);
  }

  function upgrade(version: string, mirror: string): Promise<ApiResult<InstallStatusResponse>> {
    return post<InstallStatusResponse>("kernel/install", { version, mirror }, true);
  }

  return {
    base,
    status: () => get<StatusResponse>("status"),
    changePort: (port) => post<ApiMessageResponse>("port", { port }),
    webProxyPort: () => get<{ port: number; public: boolean }>("web-proxy/port"),
    changeWebProxyPort: (port, publicAccess) => post<ApiMessageResponse & { port: number }>("web-proxy/port", { port, public: publicAccess }),
    resetAuth: () => post<ApiMessageResponse>("kernel/reset-auth"),
    uninstallKernel: () => post<ApiMessageResponse>("kernel/uninstall"),
    controlService: (action) => post<ApiMessageResponse>(action),
    getMirror: () => get<MirrorResponse>("mirror"),
    setMirror: (mirror) => post<ApiMessageResponse>("mirror", { mirror }),
    versionList: () => get<VersionListResponse>("kernel/versions"),
    upgradeStatus: () => get<InstallStatusResponse>("kernel/install-status"),
    upgrade,
    upgradeLogs: (seq, instance) => get<LogsResponse>(
      "kernel/install-log",
      `tail=40&seq=${seq}&instance=${encodeURIComponent(instance)}`,
    ),
    logs: (lines) => get<LogsResponse>("logs", `tail=${lines}`),
    clearLogs: () => post<ApiMessageResponse>("logs/clear"),
  };
}
