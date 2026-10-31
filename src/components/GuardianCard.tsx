import { useEffect, useState } from "react";
import { Activity, RefreshCw, ShieldCheck } from "lucide-react";
import { api } from "../lib/api";
import type { GuardianStatus, Site } from "../lib/types";
import { Button, Card, Pill } from "./ui";

export function GuardianCard({ sites }: { sites: Site[] }) {
  const [states, setStates] = useState<Record<string, GuardianStatus>>({});
  const [working, setWorking] = useState<string>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let alive = true;
    void Promise.all(sites.map((site) =>
      api.guardianStatus(site.id).then((status) => ({ id: site.id, status }))))
      .then((results) => { if (alive) setStates(Object.fromEntries(results.map(({ id, status }) => [id, status]))); })
      .catch((e) => { if (alive) setError((e as Error).message); });
    return () => { alive = false; };
  }, [sites.map((s) => s.id).join("|")]);

  async function action(site: Site, type: "enable" | "check") {
    setWorking(site.id); setError(undefined);
    try {
      const current = states[site.id];
      const result = type === "enable"
        ? await api.guardianEnable(site.id, !current?.enabled)
        : await api.guardianCheckNow(site.id);
      setStates((old) => ({ ...old, [site.id]: result }));
    } catch (e) { setError((e as Error).message); }
    finally { setWorking(undefined); }
  }

  if (!sites.length) return null;
  return (
    <Card className="p-4">
      <div className="mb-3 flex items-center gap-2">
        <ShieldCheck size={18} className="text-kelp" aria-hidden />
        <div className="min-w-0">
          <h2 className="text-[16px] font-semibold">Hostify Guardian</h2>
          <p className="text-[12px] text-ink2">Optional health monitoring. Automatic recovery requires three failed checks and a 15-minute cooldown.</p>
        </div>
      </div>
      <ul className="grid divide-y divide-line">
        {sites.map((site) => {
          const state = states[site.id];
          return (
            <li key={site.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{site.name}</span>
                  <Pill tone={state?.enabled ? "ok" : "idle"}>{state?.enabled ? "Monitoring" : "Disabled"}</Pill>
                </div>
                <p className="mt-1 text-[12px] text-ink2">
                  {state?.lastResult || "No health checks yet"}
                  {state?.consecutiveFailures ? ` · ${state.consecutiveFailures} consecutive failures` : ""}
                  {state?.recoveryAttempts ? ` · ${state.recoveryAttempts} recovery attempts` : ""}
                </p>
              </div>
              <div className="flex items-center gap-2">
                <Button size="sm" disabled={!!working || !state?.enabled || site.status !== "running"} onClick={() => void action(site, "check")}>
                  <Activity size={13} />Check now
                </Button>
                <Button size="sm" busy={working === site.id} disabled={!state || !!working || (!site.autoStart && !state.enabled)}
                  variant={state?.enabled ? "secondary" : "primary"}
                  onClick={() => void action(site, "enable")}>
                  {state?.enabled ? "Disable" : "Enable"}
                </Button>
              </div>
            </li>
          );
        })}
      </ul>
      {error && <p className="mt-2 text-[13px] text-buoy" role="alert">{error}</p>}
      <p className="mt-2 flex items-center gap-1 text-[12px] text-ink3"><RefreshCw size={12} />Health state updates on manual checks and through the engine activity log. Guardian never modifies DNS, source files or databases.</p>
    </Card>
  );
}
