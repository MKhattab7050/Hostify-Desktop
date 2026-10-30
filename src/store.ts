import { create } from "zustand";
import { api, listen } from "./lib/api";
import type {
  ComponentName, ActivityEntry, AppSettings, CloudflareAccount, Deployment, Domain, EngineStatus, Site, SystemStats, TunnelHealth,
} from "./lib/types";

export type Page = "dashboard" | "websites" | "domains" | "activity" | "settings";
export interface Toast { id: number; kind: "ok" | "error" | "info"; text: string }

interface State {
  page: Page;
  siteFocus?: string;
  wizardOpen: boolean;
  wizardSiteId?: string;
  engine?: EngineStatus;
  engineError?: string;
  sites: Site[];
  domains: Domain[];
  cf: CloudflareAccount;
  stats?: SystemStats;
  tunnels: TunnelHealth[];
  activity: ActivityEntry[];
  deployments: Record<string, Deployment>;
  settings?: AppSettings;
  toasts: Toast[];
  loaded: boolean;

  go: (p: Page, siteId?: string) => void;
  openWizard: (siteId?: string) => void;
  closeWizard: () => void;
  toast: (kind: Toast["kind"], text: string) => void;
  dismissToast: (id: number) => void;
  refresh: () => Promise<void>;
  refreshStats: () => Promise<void>;
  setDeployment: (d: Deployment) => void;
  installComponents: (names?: ComponentName[]) => Promise<void>;
  refreshEngine: () => Promise<void>;
  saveSettings: (s: AppSettings) => Promise<void>;
  boot: () => Promise<void>;
}

let toastId = 1;

export const useStore = create<State>((set, get) => ({
  page: "dashboard",
  wizardOpen: false,
  sites: [], domains: [], cf: { connected: false }, tunnels: [], activity: [], deployments: {}, toasts: [], loaded: false,

  go: (page, siteFocus) => set({ page, siteFocus }),
  openWizard: (wizardSiteId) => set({ wizardOpen: true, wizardSiteId }),
  closeWizard: () => { set({ wizardOpen: false, wizardSiteId: undefined }); void get().refresh(); },
  toast: (kind, text) => {
    const id = toastId++;
    set((s) => ({ toasts: [...s.toasts, { id, kind, text }] }));
    setTimeout(() => get().dismissToast(id), kind === "error" ? 9000 : 4500);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  setDeployment: (d) => set((s) => ({ deployments: { ...s.deployments, [d.id]: d } })),
  installComponents: async (names) => { set({ engine: await api.ensureComponents(names), engineError: undefined }); },
  // Status is detected live by the engine, so a re-read is all it takes to notice installs, deletions or a different engine.
  refreshEngine: async () => {
    try { set({ engine: await api.engineStatus(), engineError: undefined }); } catch (e) { set({ engineError: (e as Error).message }); }
  },

  refresh: async () => {
    const [sites, domains, cf, tunnels, activity] = await Promise.allSettled([
      api.listSites(), api.listDomains(), api.cfStatus(), api.tunnelHealth(), api.listActivity(60),
    ]);
    set((s) => ({
      sites: sites.status === "fulfilled" ? sites.value : s.sites,
      domains: domains.status === "fulfilled" ? domains.value : s.domains,
      cf: cf.status === "fulfilled" ? cf.value : s.cf,
      tunnels: tunnels.status === "fulfilled" ? tunnels.value : s.tunnels,
      activity: activity.status === "fulfilled" ? activity.value : s.activity,
    }));
  },
  refreshStats: async () => {
    try { set({ stats: await api.systemStats() }); } catch { /* engine busy; keep last reading */ }
  },
  saveSettings: async (settings) => {
    try { set({ settings: await api.saveSettings(settings) }); }
    catch (e) { get().toast("error", (e as Error).message); }
  },

  boot: async () => {
    try {
      const [engine, settings] = await Promise.all([api.engineStatus(), api.getSettings()]);
      set({ engine, settings, engineError: undefined });
    } catch (e) {
      set({ engineError: (e as Error).message });
    }
    await Promise.all([get().refresh(), get().refreshStats()]);
    set({ loaded: true });

    await listen<Site>("site-status", (site) =>
      set((s) => ({ sites: s.sites.some((x) => x.id === site.id) ? s.sites.map((x) => (x.id === site.id ? site : x)) : [site, ...s.sites] })));
    await listen<Deployment>("deployment-update", (d) => get().setDeployment(d));
    await listen<ActivityEntry>("activity", (a) => set((s) => ({ activity: [a, ...s.activity].slice(0, 200) })));

    setInterval(() => void get().refreshStats(), 3000);
    setInterval(() => void get().refreshEngine(), 5000);
    window.addEventListener("focus", () => void get().refreshEngine());
    setInterval(() => void get().refresh(), 10000);
  },
}));
