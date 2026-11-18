interface PollingLifecycleOptions {
  refresh(): void | Promise<void>;
  intervalMs: number;
  immediate?: boolean;
}

export function createPollingLifecycle({ refresh, intervalMs, immediate = false }: PollingLifecycleOptions) {
  let timer: ReturnType<typeof setInterval> | null = null;
  let active = false;
  let destroyed = false;
  let inFlight: Promise<void> | null = null;
  let pending = false;

  function reportError(error: unknown): void {
    console.error("polling refresh err:", error);
  }

  function run(): Promise<void> {
    if (destroyed || document.hidden) return Promise.resolve();
    if (inFlight) {
      pending = true;
      return inFlight;
    }
    let result: void | Promise<void>;
    try {
      result = refresh();
    } catch (error) {
      reportError(error);
      return Promise.resolve();
    }
    if (!result) return Promise.resolve();
    inFlight = Promise.resolve(result).catch(reportError).then(() => {
      inFlight = null;
      const rerun = pending;
      pending = false;
      if (rerun && !destroyed && !document.hidden) return run();
    });
    return inFlight;
  }

  function startTimer(): void {
    if (timer === null && active && !document.hidden) timer = setInterval(() => { void run(); }, intervalMs);
  }

  function stopTimer(): void {
    if (timer !== null) clearInterval(timer);
    timer = null;
  }

  function onVisibilityChange(): void {
    if (document.hidden) {
      pending = false;
      stopTimer();
      return;
    }
    if (!active) return;
    void run();
    startTimer();
  }

  document.addEventListener("visibilitychange", onVisibilityChange);

  function start(): void {
    if (active || destroyed) return;
    active = true;
    startTimer();
    if (immediate) void run();
  }

  function stop(): void {
    active = false;
    pending = false;
    stopTimer();
  }

  function destroy(): void {
    destroyed = true;
    stop();
    document.removeEventListener("visibilitychange", onVisibilityChange);
  }

  return {
    start,
    stop,
    destroy,
    refresh: run,
    get running() { return timer !== null; },
  };
}
