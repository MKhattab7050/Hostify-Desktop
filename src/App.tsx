import { useEffect, useState } from "react";
import { useStore } from "./store";
import { Sidebar } from "./components/Sidebar";
import { Skeleton, Toasts } from "./components/ui";
import { PublishWizard } from "./components/PublishWizard";
import { Onboarding } from "./components/Onboarding";
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

const ONBOARDED = "hostify-onboarded";
function readOnboarded() { try { return localStorage.getItem(ONBOARDED) === "1"; } catch { return false; } }
function writeOnboarded() { try { localStorage.setItem(ONBOARDED, "1"); } catch { /* shown again next launch; harmless */ } }

function Loading() {
  return (
    <div className="mx-auto grid max-w-[1040px] gap-4" role="status" aria-label="Loading">
      <Skeleton className="h-8 w-72" /><Skeleton className="h-20" /><Skeleton className="h-44" /><Skeleton className="h-44" />
    </div>
  );
}

export default function App() {
  const page = useStore((s) => s.page); const wizardOpen = useStore((s) => s.wizardOpen); const boot = useStore((s) => s.boot);
  const loaded = useStore((s) => s.loaded); const engineError = useStore((s) => s.engineError); const hasSites = useStore((s) => s.sites.length > 0);
  const [onboarded, setOnboarded] = useState(readOnboarded);
  useTheme();
  useEffect(() => { void boot(); }, [boot]);

  // Onboarding replaces the app rather than covering it, so nothing behind it can take focus.
  if (loaded && !onboarded && !hasSites) return <Onboarding onFinish={() => { writeOnboarded(); setOnboarded(true); }} />;

  return (
    <div className="flex h-full">
      <a href="#main" className="skip-link rounded-ctl bg-kelp px-3 py-2 font-semibold text-kelpink">Skip to content</a>
      <Sidebar />
      <main id="main" tabIndex={-1} className="min-w-0 flex-1 overflow-y-auto p-7 outline-none max-[1000px]:p-5">
        {engineError && (
          <div role="alert" className="mx-auto mb-5 max-w-[1040px] rounded-card border border-buoy bg-buoysoft p-4">
            <p className="font-semibold">Hostify can't reach its hosting engine</p>
            <p className="selectable mt-1">{engineError}</p>
            <p className="mt-1 text-ink2">Your websites may be offline. Close Hostify from the tray icon and open it again to restart the engine.</p>
          </div>
        )}
        {!loaded ? <Loading /> : (
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
