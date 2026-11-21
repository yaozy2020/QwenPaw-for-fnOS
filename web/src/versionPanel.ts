import type { ApiClient, VersionItem } from "./api";
import { shallowReactive } from "vue";

interface VersionPanelOptions {
  api: Pick<ApiClient, "versionList">;
  showToast(message: string): void;
}

export function createVersionPanel({ api, showToast }: VersionPanelOptions) {
  const view = shallowReactive({ list: [] as VersionItem[], backup: false, manual: false, selected: '', stable: '', beta: '', recommended: '', current: '', error: '', fresh: false, loading: false, actionsFor: '' });
  let listRequest = 0;
  let pendingInstallAction = '';

  function compareVersion(v1: string, v2: string): number {
    function parse(value: string): number[] {
      if (!value) return [0, 0, 0, 0, 0];
      const clean = value.replace(/^v/, "").trim();
      const match = clean.match(/^(\d+)\.(\d+)(?:\.(\d+))?(.*)$/);
      if (!match) return [0, 0, 0, 0, 0];
      const major = Number.parseInt(match[1], 10);
      const minor = Number.parseInt(match[2], 10);
      const patch = Number.parseInt(match[3] || "0", 10);
      const extra = match[4] || "";
      let stage = 4;
      let num = 0;
      if (extra.includes(".post")) {
        stage = 5;
        num = Number.parseInt(extra.replace(/\D/g, "") || "0", 10);
      } else if (extra) {
        const suffix = extra.replace(/^[.-]/, "").toLowerCase();
        if (suffix.startsWith("dev")) stage = 0;
        else if (suffix.startsWith("a") || suffix.startsWith("alpha")) stage = 1;
        else if (suffix.startsWith("b") || suffix.startsWith("beta")) stage = 2;
        else if (suffix.startsWith("rc") || suffix.startsWith("c") || suffix.startsWith("pre")) stage = 3;
        num = Number.parseInt(suffix.replace(/\D/g, "") || "0", 10);
      }
      return [major, minor, patch, stage, num];
    }
    const left = parse(v1);
    const right = parse(v2);
    for (let index = 0; index < 5; index++) {
      if (left[index] > right[index]) return 1;
      if (left[index] < right[index]) return -1;
    }
    return 0;
  }

  function listedAction(current: string, selected: string) {
    return current === view.actionsFor ? view.list.find(item => item.version === selected)?.action : undefined;
  }

  function isDowngradeOf(current: string, selected: string): boolean {
    const action = listedAction(current, selected);
    if (action) return action === "downgrade";
    return compareVersion(current, selected) > 0;
  }

  function isSameVersion(current: string, selected: string): boolean {
    if (!current || !selected) return false;
    if (listedAction(current, selected) === "reinstall") return true;
    return current.replace(/^v/i, "").trim() === selected.replace(/^v/i, "").trim();
  }

  function currentInstallAction(): string {
    if (!view.current) return "安装";
    if (isSameVersion(view.current, view.selected)) return "重新安装";
    return isDowngradeOf(view.current, view.selected) ? "降级" : "升级";
  }

  function pickVersion(version: string): void {
    if (!view.fresh || !view.list.some(item => item.version === version)) return;
    view.manual = true;
    view.selected = version;
  }

  function showVersionSourceError(detail = ""): false {
    const message = '当前下载源不可用，无法获取版本列表。请更换下载源，或检查 NAS 网络、DNS 与代理设置后重试。';
    view.error = [message, detail, view.list.length ? '当前显示上一次查询结果，不能用于安装。' : ''].filter(Boolean).join(' ');
    showToast(message);
    return false;
  }

  async function fetchVersionList(sourceChanged = false): Promise<boolean> {
    const request = ++listRequest;
    const currentAtRequest = view.current;
    if (sourceChanged) Object.assign(view, { list: [], manual: false, selected: '', stable: '', beta: '', recommended: '', error: '' });
    view.fresh = false;
    view.loading = true;
    const manual = view.manual ? view.selected : '';
    try {
      const result = await api.versionList();
      if (request !== listRequest) return false;
      const { data } = result;
      if (result.ok && data.success && Array.isArray(data.versions)) {
        view.list = data.versions;
        view.actionsFor = data.versions.find(item => item.action === 'reinstall')?.version || currentAtRequest;
        const listed = (value?: string | null) => view.list.find(v => v.version === value)?.version || "";
        view.stable = listed(data.latest_stable);
        view.beta = listed(data.latest_beta) || view.list.find(version => version.is_beta)?.version || "";
        view.recommended = listed(data.recommended_version);
        view.manual = !!listed(manual);
        view.selected = view.manual ? manual : view.recommended;
        view.error = view.recommended ? '' : view.list.length ? '当前下载源未列出本安装包带有适配补丁的版本。可从列表中选择其他版本，或更换下载源。' : '当前下载源没有可选版本。请更换下载源或稍后重试。';
        view.fresh = true;
        return true;
      }
      return showVersionSourceError(data.message || `HTTP ${result.status}`);
    } catch (error) {
      if (request !== listRequest) return false;
      console.warn("fetchVersionList err:", error);
      return showVersionSourceError((error as Error).message);
    } finally { if (request === listRequest) view.loading = false; }
  }

  async function checkOnlineVersions(): Promise<void> {
    showToast("正在在线检查 PyPI 最新版本...");
    if (await fetchVersionList()) {
      showToast(`当前下载源列出 ${view.list.length} 个版本。`);
    }
  }

  return {
    view, fetchVersionList, checkOnlineVersions, pickVersion, currentInstallAction, isSameVersion, isDowngradeOf,
    selectedVersion: () => view.fresh ? view.selected : '',
    isRecommended: () => !!view.selected && view.selected === view.recommended,
    hasVersion: (value: string) => view.fresh && view.list.some(item => item.version === value),
    currentRunningVersion: () => view.current,
    setCurrentRunningVersion: (version: string) => { view.current = version; },
    pendingInstallAction: () => pendingInstallAction,
    setPendingInstallAction: (action: string) => { pendingInstallAction = action; },
  };
}
