interface PollingLifecycleOptions {
  refresh(): void | Promise<void>;
  intervalMs: number;
  immediate?: boolean;
}

export function createPollingLifecycle({ refresh, intervalMs, immediate = false }: PollingLifecycleOptions) {
  let timer: ReturnType<typeof setInterval> | null = null;
  let active = false;

  function run(): void {
    if (!document.hidden) void refresh();
  }

  function startTimer(): void {
    if (!timer && active && !document.hidden) timer = setInterval(run, intervalMs);
  }

  function stopTimer(): void {
    if (timer) clearInterval(timer);
    timer = null;
  }

  function onVisibilityChange(): void {
    if (document.hidden) {
      stopTimer();
      return;
    }
    if (!active) return;
    run();
    startTimer();
  }

  document.addEventListener("visibilitychange", onVisibilityChange);

  function start(): void {
    if (active) return;
    active = true;
    startTimer();
    if (immediate) run();
  }

  function stop(): void {
    active = false;
    stopTimer();
  }

  function destroy(): void {
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
