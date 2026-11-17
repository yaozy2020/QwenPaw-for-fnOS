export function createMobileNavigation() {
  const sections = ["overview", "configuration", "authorization", "logs"];
  const nav = document.getElementById("mobileSectionNav");
  const links = Array.from(nav?.querySelectorAll<HTMLAnchorElement>("a[data-section][href^='#']") || []);
  const validStructure = links.length === sections.length && sections.every(section =>
    links.some(link => link.dataset.section === section && link.getAttribute("href") === `#${section}`)
    && document.getElementById(section)?.dataset.mobilePanel === section
  );
  let started = false;

  function select(section: string, updateHistory = false): void {
    const valid = sections.includes(section) || section === "version";
    const selected = section === "version" ? "overview" : valid ? section : "overview";
    const hash = `#${section === "version" ? section : selected}`;
    if (updateHistory && window.location.hash !== hash) {
      history.pushState(history.state, "", hash);
    } else if (!updateHistory && !valid && window.matchMedia("(max-width: 768px)").matches) {
      history.replaceState(history.state, "", hash);
    }
    for (const link of links) {
      if (link.dataset.section === selected) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
    document.documentElement.setAttribute("data-mobile-section", selected);
  }

  const handlers = links.map(link => ({
    link,
    click: (event: MouseEvent) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      select(link.dataset.section || "overview", true);
    },
  }));
  const syncSection = () => select(window.location.hash.slice(1));

  function stop(): void {
    if (!started) return;
    started = false;
    for (const { link, click } of handlers) link.removeEventListener("click", click);
    window.removeEventListener("hashchange", syncSection);
    window.removeEventListener("popstate", syncSection);
    document.documentElement.removeAttribute("data-mobile-section");
  }

  function start(): void {
    if (started || !validStructure) return;
    try {
      syncSection();
      for (const { link, click } of handlers) link.addEventListener("click", click);
      window.addEventListener("hashchange", syncSection);
      window.addEventListener("popstate", syncSection);
      started = true;
    } catch (error) {
      stop();
      console.error("mobile navigation err:", error);
    }
  }

  return { start, stop, select };
}
