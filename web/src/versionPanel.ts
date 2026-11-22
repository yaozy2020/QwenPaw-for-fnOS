import type { ApiClient, VersionItem } from "./api";
import { shallowReactive } from "vue";
import { t, type MessageKey } from "./i18n";

export type InstallActionKind = "install" | "reinstall" | "downgrade" | "upgrade";

const actionShortKeys: Record<InstallActionKind, MessageKey> = {
  install: "deploy.actionShort.install",
  reinstall: "deploy.actionShort.reinstall",
  downgrade: "deploy.actionShort.downgrade",
  upgrade: "deploy.actionShort.upgrade",
};

interface VersionPanelOptions {
  api: Pick<ApiClient, "versionList">;
  showToast(message: string): void;
}

const emptyText = () => "";

export function createVersionPanel({ api, showToast }: VersionPanelOptions) {
  const raw = shallowReactive({
    list: [] as VersionItem[], backup: false, manual: false, selected: '', stable: '', beta: '', recommended: '',
    current: '', errorText: emptyText as () => string, fresh: false, loading: false, actionsFor: '',
  });
  const view = {
    get list() { return raw.list; },
    get backup() { return raw.backup; },
    set backup(value: boolean) { raw.backup = value; },
    get manual() { return raw.manual; },
    get selected() { return raw.selected; },
    get stable() { return raw.stable; },
    get beta() { return raw.beta; },
    get recommended() { return raw.recommended; },
    get current() { return raw.current; },
    get error() { return raw.errorText(); },
    get fresh() { return raw.fresh; },
    get loading() { return raw.loading; },
    get actionsFor() { return raw.actionsFor; },
  };
  let listRequest = 0;
  let pendingInstallAction: InstallActionKind | '' = '';

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
    return current === raw.actionsFor ? raw.list.find(item => item.version === selected)?.action : undefined;
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

  function currentInstallActionKind(): InstallActionKind {
    if (!raw.current) return "install";
    if (isSameVersion(raw.current, raw.selected)) return "reinstall";
    return isDowngradeOf(raw.current, raw.selected) ? "downgrade" : "upgrade";
  }

  function currentInstallAction(): string {
    return t(actionShortKeys[currentInstallActionKind()]);
  }

  function pickVersion(version: string): void {
    if (!raw.fresh || !raw.list.some(item => item.version === version)) return;
    raw.manual = true;
    raw.selected = version;
  }

  function showVersionSourceError(detail = ""): false {
    const hasPrevious = raw.list.length > 0;
    raw.errorText = () => [t("version.sourceUnavailable"), detail, hasPrevious ? t("version.staleWarning") : ''].filter(Boolean).join(' ');
    showToast(t("version.sourceUnavailable"));
    return false;
  }

  async function fetchVersionList(sourceChanged = false): Promise<boolean> {
    const request = ++listRequest;
    const currentAtRequest = raw.current;
    if (sourceChanged) Object.assign(raw, { list: [], manual: false, selected: '', stable: '', beta: '', recommended: '', errorText: emptyText });
    raw.fresh = false;
    raw.loading = true;
    const manual = raw.manual ? raw.selected : '';
    try {
      const result = await api.versionList();
      if (request !== listRequest) return false;
      const { data } = result;
      if (result.ok && data.success && Array.isArray(data.versions)) {
        raw.list = data.versions;
        raw.actionsFor = data.versions.find(item => item.action === 'reinstall')?.version || currentAtRequest;
        const listed = (value?: string | null) => raw.list.find(v => v.version === value)?.version || "";
        raw.stable = listed(data.latest_stable);
        raw.beta = listed(data.latest_beta) || raw.list.find(version => version.is_beta)?.version || "";
        raw.recommended = listed(data.recommended_version);
        raw.manual = !!listed(manual);
        raw.selected = raw.manual ? manual : raw.recommended;
        raw.errorText = raw.recommended
          ? emptyText
          : raw.list.length
            ? () => t("version.noRecommended")
            : () => t("version.emptyList");
        raw.fresh = true;
        return true;
      }
      return showVersionSourceError(data.message || `HTTP ${result.status}`);
    } catch (error) {
      if (request !== listRequest) return false;
      console.warn("fetchVersionList err:", error);
      return showVersionSourceError((error as Error).message);
    } finally { if (request === listRequest) raw.loading = false; }
  }

  async function checkOnlineVersions(): Promise<void> {
    showToast(t("version.checkingToast"));
    if (await fetchVersionList()) {
      showToast(t("version.listedToast", { count: raw.list.length }));
    }
  }

  return {
    view, fetchVersionList, checkOnlineVersions, pickVersion, currentInstallAction, currentInstallActionKind, isSameVersion, isDowngradeOf,
    selectedVersion: () => raw.fresh ? raw.selected : '',
    isRecommended: () => !!raw.selected && raw.selected === raw.recommended,
    hasVersion: (value: string) => raw.fresh && raw.list.some(item => item.version === value),
    currentRunningVersion: () => raw.current,
    setCurrentRunningVersion: (version: string) => { raw.current = version; },
    pendingInstallAction: () => pendingInstallAction ? t(actionShortKeys[pendingInstallAction]) : '',
    setPendingInstallAction: (action: InstallActionKind | '') => { pendingInstallAction = action; },
  };
}
