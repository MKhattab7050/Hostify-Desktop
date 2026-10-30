import { Activity, Globe2, LayoutDashboard, Rocket, Settings, Server } from "lucide-react";
import { useStore, type Page } from "../store";
import { cx } from "./ui";
import { isTauri } from "../lib/api";

const NAV: { id: Page; label: string; icon: typeof Activity }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "websites", label: "Websites", icon: Server },
  { id: "domains", label: "Domains", icon: Globe2 },
  { id: "activity", label: "Activity & Logs", icon: Activity },
  { id: "settings", label: "Settings", icon: Settings },
];

export function Sidebar() {
  const { page, go, openWizard, engine } = useStore();
  return (
    <aside className="flex w-[220px] shrink-0 flex-col border-r border-line bg-panel max-[1000px]:w-[68px]">
      <div className="flex items-center gap-2.5 px-4 py-5 max-[1000px]:justify-center max-[1000px]:px-0">
        <div className="grid h-8 w-8 place-items-center rounded-lg bg-kelp text-kelpink" aria-hidden>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round"><path d="M4 18V8l8 5 8-5v10" /><circle cx="12" cy="6" r="1.6" fill="currentColor" stroke="none" /></svg>
        </div>
        <span className="font-display text-xl font-bold tracking-tight max-[1000px]:hidden">Hostify</span>
      </div>

      <div className="px-3 max-[1000px]:px-2">
        <button onClick={() => openWizard()}
          className="flex h-10 w-full items-center justify-center gap-2 rounded-ctl bg-kelp font-semibold text-kelpink transition hover:brightness-110">
          <Rocket size={16} aria-hidden /><span className="max-[1000px]:hidden">Publish website</span>
          <span className="sr-only min-[1001px]:hidden">Publish website</span>
        </button>
      </div>

      <nav className="mt-4 grid gap-0.5 px-3 max-[1000px]:px-2" aria-label="Main">
        {NAV.map(({ id, label, icon: Icon }) => (
          <button key={id} onClick={() => go(id)} aria-current={page === id ? "page" : undefined} title={label}
            className={cx("flex h-10 items-center gap-3 rounded-ctl px-3 text-left transition-colors max-[1000px]:justify-center max-[1000px]:px-0",
              page === id ? "bg-kelpsoft font-semibold text-kelp" : "text-ink2 hover:bg-panel2 hover:text-ink")}>
            <Icon size={18} aria-hidden /><span className="max-[1000px]:hidden">{label}</span>
            <span className="sr-only min-[1001px]:hidden">{label}</span>
          </button>
        ))}
      </nav>

      <div className="mt-auto border-t border-line p-4 text-[12px] text-ink3 max-[1000px]:hidden">
        <div className="flex items-center gap-2">
          <span className={cx("h-2 w-2 rounded-full", engine?.running ? "bg-kelp" : "bg-buoy")} />
          {engine?.running ? "Hosting engine running" : "Hosting engine offline"}
        </div>
        {!isTauri && <p className="mt-1.5 text-lantern">Design preview — sample data</p>}
      </div>
    </aside>
  );
}
