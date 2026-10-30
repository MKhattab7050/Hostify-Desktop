import { useEffect } from "react";
import { useStore } from "./store";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/ui";
import { PublishWizard } from "./components/PublishWizard";
import { Dashboard } from "./pages/Dashboard";
import { Websites } from "./pages/Websites";
import { Domains } from "./pages/Domains";
import { Activity } from "./pages/Activity";
import { Settings } from "./pages/Settings";

function useTheme() {
  const theme = useStore((s) => s.settings?.theme ?? "system");
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => document.documentElement.setAttribute("data-theme", theme === "system" ? (mq.matches ? "dark" : "light") : theme);
    apply(); mq.addEventListener("change", apply); return () => mq.removeEventListener("change", apply);
  }, [theme]);
}

export default function App() {
  const { page, wizardOpen, boot, loaded, engineError } = useStore();
  useTheme();
  useEffect(() => { void boot(); }, [boot]);

  return (
    <div className="flex h-full">
      <Sidebar />
      <main className="min-w-0 flex-1 overflow-y-auto p-8 max-[1000px]:p-5">
        {engineError && (
          <div role="alert" className="mx-auto mb-5 max-w-[1100px] rounded-card border border-buoy bg-buoysoft p-4">
            <p className="font-semibold text-buoy">Hostify can't reach its hosting engine</p>
            <p className="mt-1">{engineError} Your websites may be offline. Restart Hostify to start the engine again.</p>
          </div>
        )}
        {!loaded ? <p className="text-ink2">Starting…</p> : (
          <div key={page} className="rise">
            {page === "dashboard" && <Dashboard />}
            {page === "websites" && <Websites />}
            {page === "domains" && <Domains />}
            {page === "activity" && <Activity />}
            {page === "settings" && <Settings />}
          </div>
        )}
      </main>
      {wizardOpen && <PublishWizard />}
      <Toasts />
    </div>
  );
}
