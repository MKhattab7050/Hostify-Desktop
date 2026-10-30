import { Activity, Boxes, Database, Globe2, LayoutDashboard, Plus, Rocket, Settings, Server } from "lucide-react";
import { useStore, type Page } from "../store";
import { cx } from "./ui";
import { isTauri } from "../lib/api";

const NAV: { id: Page; label: string; icon: typeof Activity }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "websites", label: "Websites", icon: Server },
  { id: "deployments", label: "Deployments", icon: Rocket },
  { id: "domains", label: "Domains", icon: Globe2 },
  { id: "databases", label: "Databases", icon: Database },
  { id: "runtimes", label: "Runtimes", icon: Boxes },
  { id: "activity", label: "Activity & Logs", icon: Activity },
  { id: "settings", label: "Settings", icon: Settings },
];

export function Logo() {
  return (
    <div className="grid h-8 w-8 shrink-0 place-items-center rounded-lg bg-kelp text-kelpink" aria-hidden>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round"><path d="M4 18V8l8 5 8-5v10" /><circle cx="12" cy="6" r="1.6" fill="currentColor" stroke="none" /></svg>
    </div>
  );
}

export function Sidebar() {
  const page = useStore((s) => s.page); const go = useStore((s) => s.go); const openWizard = useStore((s) => s.openWizard);
  const running = useStore((s) => !!s.engine?.running);
  const waiting = useStore((s) => s.sites.some((x) => x.publishState === "recovery_required" || x.status === "error"));
  return (
    <aside className="flex w-[216px] shrink-0 flex-col border-r border-line bg-panel max-[1000px]:w-[60px]">
      <div className="flex items-center gap-2.5 px-4 pb-4 pt-5 max-[1000px]:justify-center max-[1000px]:px-0">
        <Logo />
        <span className="font-display text-xl font-bold tracking-tight max-[1000px]:hidden">Hostify</span>
      </div>

      <nav className="grid gap-0.5 px-2.5" aria-label="Main">
        {NAV.map(({ id, label, icon: Icon }) => {
          const on = page === id;
          return (
            <button key={id} type="button" onClick={() => go(id)} aria-current={on ? "page" : undefined} title={label}
              className={cx("press relative flex h-9 items-center gap-2.5 rounded-ctl px-2.5 text-left max-[1000px]:justify-center max-[1000px]:px-0",
                on ? "bg-panel2 font-semibold text-ink" : "text-ink2 hover:bg-panel2 hover:text-ink")}>
              {on && <span className="absolute -left-2.5 top-2 h-5 w-[3px] rounded-r bg-kelp" aria-hidden />}
              <Icon size={17} aria-hidden className={on ? "text-kelp" : undefined} /><span className="max-[1000px]:sr-only">{label}</span>
              {id === "websites" && waiting && <span className="ml-auto h-1.5 w-1.5 rounded-full bg-buoy max-[1000px]:absolute max-[1000px]:right-2 max-[1000px]:top-2" role="img" aria-label="A website needs attention" />}
            </button>
          );
        })}
      </nav>

      <div className="mt-3 px-2.5">
        <button type="button" onClick={() => openWizard()} title="Add website"
          className="press flex h-9 w-full items-center justify-center gap-2 rounded-ctl border border-line text-ink hover:border-linestrong hover:bg-panel2">
          <Plus size={15} aria-hidden /><span className="max-[1000px]:sr-only">Add website</span>
        </button>
      </div>

      <div className="mt-auto flex items-center gap-2 border-t border-line p-4 text-[12px] text-ink2 max-[1000px]:justify-center max-[1000px]:px-0">
        <span className={cx("h-2 w-2 shrink-0 rounded-full", running ? "bg-kelp" : "bg-buoy")} aria-hidden />
        <span className="max-[1000px]:sr-only">{running ? "Hosting engine running" : "Hosting engine offline"}</span>
      </div>
      {!isTauri && <p className="px-4 pb-3 text-[12px] text-lantern max-[1000px]:hidden">Design preview with sample data</p>}
    </aside>
  );
}
