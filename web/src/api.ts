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
}

export type ServiceAction = "start" | "stop" | "restart";

export function deriveApiBase(pathname: string): string {
  if (pathname.includes("index.cgi")) {
    return pathname.substring(0, pathname.indexOf("index.cgi")) + "api.cgi";
  }
  return "/cgi/ThirdParty/qwenpaw/api.cgi";
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
  resetAuth(): Promise<ApiResult<ApiMessageResponse>>;
  uninstallKernel(): Promise<ApiResult<ApiMessageResponse>>;
  controlService(action: ServiceAction): Promise<ApiResult<ApiMessageResponse>>;
  getMirror(): Promise<ApiResult<MirrorResponse>>;
  setMirror(mirror: string): Promise<ApiResult<ApiMessageResponse>>;
  versionList(): Promise<ApiResult<VersionListResponse>>;
  upgradeStatus(): Promise<ApiResult<InstallStatusResponse>>;
  upgrade(version: string, mirror: string): Promise<ApiResult<InstallStatusResponse>>;
  upgradeBeaconUrl(version: string, mirror: string): string;
  upgradeLogs(): Promise<ApiResult<LogsResponse>>;
  logs(lines: number): Promise<ApiResult<LogsResponse>>;
  clearLogs(): Promise<ApiResult<ApiMessageResponse>>;
}

export function createApiClient(options: ApiClientOptions): ApiClient {
  const base = deriveApiBase(options.pathname);
  const cookie = options.cookie || (() => typeof document === "undefined" ? "" : document.cookie);
  const fetcher = options.fetch || globalThis.fetch.bind(globalThis);
  const now = options.now || Date.now;

  function csrfHeaders(): Record<string, string> {
    const token = readCsrfToken(cookie());
    return token ? { "X-CSRF-Token": token } : {};
  }

  function upgradeQuery(version: string, mirror: string): string {
    return `action=upgrade&version=${encodeURIComponent(version)}&mirror=${encodeURIComponent(mirror)}`;
  }

  async function request<T>(query: string, init?: RequestInit): Promise<ApiResult<T>> {
    let response: Response;
    try {
      response = await fetcher(`${base}?${query}`, init);
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
  }

  function get<T>(action: string, params = ""): Promise<ApiResult<T>> {
    return request<T>(`action=${action}${params}&_t=${now()}`);
  }

  function postQuery<T>(query: string, init: RequestInit = {}): Promise<ApiResult<T>> {
    const headers = new Headers(init.headers);
    for (const [name, value] of Object.entries(csrfHeaders())) {
      if (!headers.has(name)) headers.set(name, value);
    }
    return request<T>(query, { ...init, method: "POST", headers });
  }

  function post<T>(action: string, params = "", init: RequestInit = {}): Promise<ApiResult<T>> {
    return postQuery<T>(`action=${action}${params}`, init);
  }

  return {
    base,
    status: () => get<StatusResponse>("status"),
    changePort: (port) => post<ApiMessageResponse>("change_port", `&port=${port}`),
    resetAuth: () => post<ApiMessageResponse>("reset_auth"),
    uninstallKernel: () => post<ApiMessageResponse>("uninstall_kernel"),
    controlService: (action) => post<ApiMessageResponse>(action),
    getMirror: () => get<MirrorResponse>("get_mirror"),
    setMirror: (mirror) => post<ApiMessageResponse>("set_mirror", `&mirror=${encodeURIComponent(mirror)}`),
    versionList: () => get<VersionListResponse>("version_list"),
    upgradeStatus: () => get<InstallStatusResponse>("upgrade_status"),
    upgrade: (version, mirror) => postQuery<InstallStatusResponse>(upgradeQuery(version, mirror), { keepalive: true }),
    upgradeBeaconUrl: (version, mirror) => `${base}?${upgradeQuery(version, mirror)}&_csrf=${encodeURIComponent(readCsrfToken(cookie()))}`,
    upgradeLogs: () => get<LogsResponse>("upgrade_logs"),
    logs: (lines) => get<LogsResponse>("logs", `&lines=${lines}`),
    clearLogs: () => post<ApiMessageResponse>("clear_logs"),
  };
}
