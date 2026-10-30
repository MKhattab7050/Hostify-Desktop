import { useCallback, useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Button, Card, Disclosure, Skeleton, cx, fmtBytes, fmtTime } from "./ui";
import { BuildPanel } from "./BuildPanel";
import { AppConfigForm } from "./AppConfigForm";
import { EnvVars } from "./EnvVars";
import type { AppConfig, AppStatus, LogStream, RuntimeStatus, Site } from "../lib/types";

const KIND_NAME = { node: "Node.js", php: "PHP" } as const;

function Metric({ k, v }: { k: string; v?: string }) {
  // Nothing measured means nothing shown: an empty value stays a dash, never a zero.
  return <div className="bg-panel p-3"><div className="text-[13px] text-ink3">{k}</div><div className="tnum selectable mt-0.5 font-medium">{v ?? "–"}</div></div>;
}

function Status({ site }: { site: Site }) {
  const [s, setS] = useState<AppStatus>(); const [err, setErr] = useState<string>();
  useEffect(() => {
    let live = true;
    const load = () => { if (document.hidden) return; api.appStatus(site.id).then((v) => { if (live) { setS(v); setErr(undefined); } }).catch((e) => live && setErr((e as Error).message)); };
    load(); const t = setInterval(load, 5000);
    return () => { live = false; clearInterval(t); };
  }, [site.id, site.status]);
  if (err) return <p role="alert" className="selectable text-[13px] text-buoy">{err}</p>;
  if (!s) return <Skeleton className="h-16" />;
  return (
    <div className="grid gap-2">
      <div className="grid gap-px overflow-hidden rounded-ctl border border-line bg-line sm:grid-cols-3 md:grid-cols-5">
        <Metric k="Processor" v={s.cpuPercent !== undefined ? `${s.cpuPercent.toFixed(1)}%` : undefined} />
        <Metric k="Memory" v={s.memoryBytes !== undefined ? fmtBytes(s.memoryBytes) : undefined} />
        <Metric k="Processes" v={s.processCount !== undefined ? String(s.processCount) : undefined} />
        <Metric k="Restarts" v={s.restarts !== undefined ? String(s.restarts) : undefined} />
        <Metric k="Last exit code" v={s.lastExitCode !== undefined ? String(s.lastExitCode) : undefined} />
      </div>
      {s.lastExitAt && <p className="text-[13px] text-ink3">Last stopped {fmtTime(s.lastExitAt)}</p>}
      {s.workers && s.workers.some((w) => w.health !== "running") && <p className="text-[13px] text-lantern">{s.workers.filter((w) => w.health !== "running").length} of {s.workers.length} PHP processes aren't responding.</p>}
      {s.exposedAddresses.length > 0 && (
        <p role="alert" className="flex gap-2 rounded-ctl bg-lanternsoft p-2.5 text-[13px]"><AlertTriangle size={15} className="mt-0.5 shrink-0 text-lantern" aria-hidden />
          This app chose to listen on {s.exposedAddresses.join(", ")}, so other computers on your network may be able to reach it directly. Change the app to use the HOST variable Hostify provides.</p>)}
    </div>
  );
}

function Logs({ site }: { site: Site }) {
  const [stream, setStream] = useState<LogStream>("app"); const [lines, setLines] = useState<string[]>(); const [err, setErr] = useState<string>();
  useEffect(() => {
    let live = true; setLines(undefined);
    const load = () => { if (document.hidden) return; api.appLogs(site.id, stream, 300).then((l) => { if (live) { setLines(l); setErr(undefined); } }).catch((e) => live && setErr((e as Error).message)); };
    load(); const t = setInterval(load, 5000);
    return () => { live = false; clearInterval(t); };
  }, [site.id, stream]);
  const tabs: [LogStream, string][] = [["app", "App output"], ["build", "Build"], ["access", "Requests"]];
  return (
    <div className="grid gap-2">
      <div role="tablist" aria-label="Log" className="flex gap-1">
        {tabs.map(([id, label]) => (
          <button key={id} role="tab" type="button" aria-selected={stream === id} onClick={() => setStream(id)}
            className={cx("press rounded-ctl px-2.5 py-1 text-[13px]", stream === id ? "bg-panel2 font-semibold" : "text-ink2 hover:bg-panel2")}>{label}</button>))}
      </div>
      {err && <p role="alert" className="selectable text-[13px] text-buoy">{err}</p>}
      {lines ? <pre className="selectable max-h-72 overflow-auto whitespace-pre-wrap break-all rounded-ctl bg-chalk p-3 font-mono text-[12px] leading-5 text-ink2">{lines.length ? lines.join("\n") : "Nothing logged yet."}</pre> : !err && <Skeleton className="h-24" />}
    </div>
  );
}

function Settings({ site, onRebuild }: { site: Site; onRebuild: () => void }) {
  const toast = useStore((s) => s.toast);
  const [config, setConfig] = useState<AppConfig>(); const [versions, setVersions] = useState<RuntimeStatus[]>(); const [version, setVersion] = useState(site.runtimeVersion ?? "");
  const [busy, setBusy] = useState(false); const [note, setNote] = useState<string>(); const [err, setErr] = useState<string>();
  const kind = site.kind === "php" ? "php" : "node";
  const load = useCallback(() => { api.getAppConfig(site.id).then(setConfig).catch((e) => setErr((e as Error).message)); api.listRuntimes().then((l) => setVersions(l.filter((r) => r.kind === kind))).catch((e) => setErr((e as Error).message)); }, [site.id, kind]);
  useEffect(load, [load]);
  async function save() {
    if (!config) return; setBusy(true); setErr(undefined); setNote(undefined);
    try {
      const r = await api.setAppConfig(site.id, config, kind === "node" ? { nodeVersion: version } : { phpVersion: version });
      setNote(r.rebuildRequired ? "Saved. Build again to use these settings." : r.restartRequired ? "Saved. Restart the website to use these settings." : "Saved.");
      if (r.rebuildRequired) onRebuild();
      toast("ok", "Application settings saved");
    } catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }
  if (!config || !versions) return err ? <p role="alert" className="selectable text-buoy">{err}</p> : <Skeleton className="h-24" />;
  return (
    <div className="grid gap-4">
      <label className="grid gap-1.5"><span className="text-[13px] font-medium">{KIND_NAME[kind]} version</span>
        <select value={version} onChange={(e) => setVersion(e.target.value)} className="h-9 rounded-ctl border border-line bg-panel px-2">
          {versions.map((r) => <option key={r.version} value={r.version} disabled={!r.ready}>{r.version}{r.ready ? "" : " (install it on the Runtimes page)"}</option>)}</select></label>
      <AppConfigForm kind={kind} config={config} onChange={setConfig} />
      {err && <p role="alert" className="selectable text-buoy">{err}</p>}
      {note && <p role="status" className="text-[13px] text-ink2">{note}</p>}
      <div><Button variant="primary" busy={busy} onClick={save}>Save settings</Button></div>
    </div>
  );
}

/** Application details for server-side websites. Static websites never render this. */
export function AppDetails({ site }: { site: Site }) {
  const [buildKey, setBuildKey] = useState(0);
  const kind = site.kind === "php" ? "php" : "node";
  return (
    <>
      <Card className="grid gap-3 p-4">
        <div className="flex flex-wrap items-baseline justify-between gap-2"><h3 className="text-[16px] font-semibold">{KIND_NAME[kind]} application</h3>
          <span className="text-[13px] text-ink2">{site.runtimeVersion ? `${KIND_NAME[kind]} ${site.runtimeVersion}` : "No runtime chosen"}</span></div>
        <Status site={site} />
        <BuildPanel key={buildKey} site={site} compact={false} />
      </Card>
      <Card className="p-4"><Disclosure title="Logs" defaultOpen><div className="mt-2"><Logs site={site} /></div></Disclosure></Card>
      <Card className="p-4"><Disclosure title="Environment variables"><div className="mt-2"><EnvVars site={site} /></div></Disclosure></Card>
      <Card className="p-4"><Disclosure title="Application settings"><div className="mt-2"><Settings site={site} onRebuild={() => setBuildKey((k) => k + 1)} /></div></Disclosure></Card>
    </>
  );
}
