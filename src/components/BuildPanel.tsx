import { useCallback, useEffect, useState } from "react";
import { Check, Circle, Loader2, ShieldAlert, X } from "lucide-react";
import { useStore } from "../store";
import { api, listen } from "../lib/api";
import { Button, cx } from "./ui";
import type { AppConfig, BuildStatus, Site } from "../lib/types";

const ORDER: BuildStatus["step"][] = ["install", "build", "start", "verify"];
const LABEL: Record<BuildStatus["step"], string> = { install: "Install dependencies", build: "Build", start: "Start the app", verify: "Check it through the website server" };

export function isBuilt(c?: AppConfig) { return !!c?.builtRelease && (c.builtRelease === c.pendingRelease || (!c.pendingRelease && c.builtRelease === c.activeRelease)); }

/** Only ever reflects what the engine reports. A step is ticked because the engine said so. */
function Steps({ status, kind, config }: { status?: BuildStatus | null; kind?: string; config?: AppConfig }) {
  const shown = ORDER.filter((s) => (s === "install" ? kind === "node" && config?.install !== "none" : s === "build" ? kind === "node" && !!config?.buildScript : status?.step === s || ORDER.indexOf(status?.step ?? "install") > ORDER.indexOf(s)));
  if (!shown.length) return null;
  const current = status ? ORDER.indexOf(status.step) : -1;
  return (
    <ol className="grid gap-1.5">
      {shown.map((s) => {
        const i = ORDER.indexOf(s);
        const state = i < current ? "done" : i > current ? "pending" : status?.state ?? "pending";
        return (
          <li key={s} className="flex items-center gap-2">
            {state === "done" ? <Check size={15} className="text-kelp" aria-label="Done" /> : state === "running" ? <Loader2 size={15} className="spin text-lantern" aria-label="Running" />
              : state === "failed" || state === "cancelled" ? <X size={15} className="text-buoy" aria-label={state === "failed" ? "Failed" : "Cancelled"} /> : <Circle size={15} className="text-ink3" aria-label="Not started" />}
            <span className={cx(state === "pending" && "text-ink3")}>{LABEL[s]}</span>
          </li>);
      })}
    </ol>
  );
}

export function TrustNotice({ site, lifecycle, ack, onAck }: { site: Pick<Site, "name" | "kind">; lifecycle?: Record<string, string>; ack: boolean; onAck: (v: boolean) => void }) {
  const names = Object.entries(lifecycle ?? {});
  return (
    <div className="grid gap-2 rounded-card border border-lantern bg-lanternsoft p-3.5">
      <p className="flex items-center gap-2 font-semibold"><ShieldAlert size={16} className="text-lantern" aria-hidden />Only continue if you trust this project</p>
      <p>Hostify will run this project's code on this PC as your Windows account. It can read and change your files and use your network, and Hostify can't contain it. Publishing connects it to the internet.</p>
      {names.length > 0 && <div><p className="text-[13px] font-medium">This project's own install scripts</p>
        <ul className="mt-1 grid gap-0.5">{names.map(([k, v]) => <li key={k} className="selectable break-all font-mono text-[12px]">{k}: {v}</li>)}</ul></div>}
      <label className="mt-1 flex items-start gap-2.5"><input type="checkbox" className="mt-1" checked={ack} onChange={(e) => onAck(e.target.checked)} /><span>I trust {site.name} and want to run its code on this PC</span></label>
    </div>
  );
}

export function BuildPanel({ site, lifecycle, onBuilt, compact }: { site: Site; lifecycle?: Record<string, string>; onBuilt?: () => void; compact?: boolean }) {
  const toast = useStore((s) => s.toast);
  const [status, setStatus] = useState<BuildStatus | null>(); const [config, setConfig] = useState<AppConfig>(); const [log, setLog] = useState<string[]>([]);
  const [ack, setAck] = useState(false); const [starting, setStarting] = useState(false);
  const running = status?.state === "running";
  const sync = useCallback(async () => {
    const [s, c] = await Promise.all([api.getBuildStatus(site.id), api.getAppConfig(site.id)]);
    setStatus(s); setConfig(c);
    return { s, c };
  }, [site.id]);

  useEffect(() => { sync().catch((e) => toast("error", (e as Error).message)); }, [sync, toast]);
  useEffect(() => {
    let off: (() => void) | undefined; let dead = false;
    listen<BuildStatus>("app-build", (s) => { if (s.siteId === site.id) { setStatus(s); void sync(); } }).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, [site.id, sync]);
  // The log and status follow on a short interval only while something is running.
  useEffect(() => {
    if (!running) return;
    const load = () => { api.appLogs(site.id, "build", 200).then(setLog).catch(() => {}); void sync(); };
    load(); const t = setInterval(load, 1500);
    return () => clearInterval(t);
  }, [running, site.id, sync]);
  const built = isBuilt(config);
  useEffect(() => { if (built && status?.state === "done") onBuilt?.(); }, [built, status?.state, onBuilt]);

  async function build() {
    setStarting(true);
    try { await api.buildApp(site.id); setLog([]); setAck(false); await sync(); }
    catch (e) { toast("error", (e as Error).message); } finally { setStarting(false); }
  }
  async function cancel() { try { await api.cancelBuild(site.id); await sync(); } catch (e) { toast("error", (e as Error).message); } }

  const failed = status?.state === "failed" || status?.state === "cancelled";
  const pendingWork = !!config?.pendingRelease && config.builtRelease !== config.pendingRelease;
  const needsBuild = !running && (pendingWork || !built);
  return (
    <div className="grid gap-3">
      {!compact && config?.pendingRelease && config.activeRelease && <p className="text-[13px] text-ink2">A new version is waiting. The current version keeps serving until the new one is built and passes its check.</p>}
      {status && <Steps status={status} kind={site.kind} config={config} />}
      {failed && status && <div role="alert" className="rounded-card border border-buoy bg-buoysoft p-3.5">
        <p className="font-medium text-buoy">{status.state === "cancelled" ? "The build was cancelled" : "The build failed"}{status.exitCode !== null ? ` with exit code ${status.exitCode}` : ""}</p>
        <pre className="selectable mt-1.5 max-h-48 overflow-auto whitespace-pre-wrap break-all font-mono text-[12px] leading-5">{status.detail}</pre></div>}
      {status?.state === "done" && built && <p role="status" className="flex items-center gap-2"><Check size={16} className="text-kelp" aria-hidden />{status.detail}</p>}
      {running && log.length > 0 && <pre aria-label="Build log" className="selectable max-h-52 overflow-auto whitespace-pre-wrap break-all rounded-ctl bg-chalk p-3 font-mono text-[12px] leading-5 text-ink2">{log.join("\n")}</pre>}
      {needsBuild && config && <TrustNotice site={site} lifecycle={lifecycle} ack={ack} onAck={setAck} />}
      <div className="flex flex-wrap gap-2">
        {running && <Button onClick={cancel}>Cancel build</Button>}
        {needsBuild && config && <Button variant="primary" busy={starting} disabled={!ack} onClick={build}>{site.kind === "php" ? "Trust and prepare" : failed ? "Trust and build again" : "Trust and build"}</Button>}
      </div>
    </div>
  );
}
