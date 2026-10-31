import { useState } from "react";
import { api } from "../lib/api";
import { useStore } from "../store";
import type { Site } from "../lib/types";

async function smartStartOrRestart(site: Site, restart = false) {
  // Starting a site must never auto-run dependency scripts or grant code trust.
  // The Build panel provides the explicit approval and verifiable build path.
  return restart ? api.restartSite(site.id) : api.startSite(site.id);
}

/** Start/stop/restart/open with per-site busy tracking and honest error toasts. */
export function useSiteActions() {
  const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const [busy, setBusy] = useState<Record<string, string | undefined>>({});

  async function run(site: Site, key: string, fn: () => Promise<unknown>, okText?: string) {
    setBusy((b) => ({ ...b, [site.id]: key }));
    try { await fn(); await refresh(); if (okText) toast("ok", okText); }
    catch (e) { toast("error", `${site.name}: ${(e as Error).message}`); }
    finally { setBusy((b) => ({ ...b, [site.id]: undefined })); }
  }
  return {
    busy,
    start: (s: Site) => run(s, "start", () => smartStartOrRestart(s, false), `${s.name} started`),
    stop: (s: Site) => run(s, "stop", () => api.stopSite(s.id), `${s.name} stopped`),
    restart: (s: Site) => run(s, "restart", () => smartStartOrRestart(s, true), `${s.name} restarted`),
    preview: (s: Site) => run(s, "preview", async () => api.openExternal(await api.previewUrl(s.id))),
    openPublic: (s: Site) => s.publicUrl && api.openExternal(s.publicUrl),
  };
}
