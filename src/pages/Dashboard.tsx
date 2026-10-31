import { useEffect, useState, type ReactNode } from "react";
import { ExternalLink, Play, Plus, Rocket, RotateCw, Square } from "lucide-react";
import { listen } from "../lib/api";
import type { ComponentName } from "../lib/types";
import { tunnelFor, useStore } from "../store";
import { publishPill } from "../lib/publish";
import { Button, Card, Empty, IconButton, Meter, PageHeader, Pill, Skeleton, StatusPill, fmtBytes, fmtTime, cx } from "../components/ui";
import { SignalPath } from "../components/SignalPath";
import { GuardianCard } from "../components/GuardianCard";
import { TimeMachineCard } from "../components/TimeMachineCard";
import { useSiteActions } from "../components/siteActions";

function Tile({ label, children, sub }: { label: string; children: ReactNode; sub?: string }) {
  return (
    <div className="bg-panel px-4 py-3">
      <div className="text-[13px] text-ink2">{label}</div>
      <div className="tnum mt-0.5 font-display text-[22px] font-semibold leading-tight">{children}</div>
      {sub && <div className="mt-0.5 truncate text-[12px] text-ink3" title={sub}>{sub}</div>}
    </div>
  );
}

export function Dashboard() {
  const sites = useStore((s) => s.sites); const stats = useStore((s) => s.stats); const tunnels = useStore((s) => s.tunnels);
  const activity = useStore((s) => s.activity); const openWizard = useStore((s) => s.openWizard); const go = useStore((s) => s.go);
  const engine = useStore((s) => s.engine); const domains = useStore((s) => s.domains);
  const act = useSiteActions();
  const online = sites.filter((s) => s.status === "running").length;
  const published = sites.filter((s) => s.publishState === "published" || s.publicUrl).length;
  const connected = tunnels.filter((t) => t.connected).length;

  return (
    <div className="mx-auto grid max-w-[1040px] gap-5">
      <PageHeader title={sites.length === 0 ? "Put your first website online" : `${online} of ${sites.length} ${sites.length === 1 ? "website" : "websites"} running`}
        sub={tunnels.length === 0 ? "No tunnels yet. One is created the first time you publish." : connected === tunnels.length ? `${connected === 1 ? "Your tunnel is" : `All ${connected} tunnels are`} connected to Cloudflare.` : `${tunnels.length - connected} of ${tunnels.length} tunnels disconnected. Hostify is reconnecting.`}>
        <Button variant="primary" onClick={() => openWizard()}><Plus size={15} aria-hidden />Add website</Button>
      </PageHeader>

      {engine && <ComponentsBanner />}

      {sites.length > 0 && (
        <Card className="grid grid-cols-5 gap-px overflow-hidden bg-line max-[1100px]:grid-cols-3">
          <Tile label="Websites">{sites.length}</Tile>
          <Tile label="Published">{published}</Tile>
          <Tile label="Local only">{sites.length - published}</Tile>
          <Tile label="Hosting engine" sub={engine ? `Version ${engine.version}` : undefined}>{engine?.running ? "Running" : "Offline"}</Tile>
          <Tile label="Tunnels" sub={tunnels.length ? `${tunnels.reduce((n, t) => n + t.connections, 0)} connections` : "None yet"}>{connected}/{tunnels.length}</Tile>
        </Card>
      )}

      {sites.length === 0 ? (
        <Card>
          <Empty icon={<Rocket size={20} />} title="No websites yet"
            body="Pick a folder or drop a ZIP. You can preview it here before anyone else can see it."
            action={<Button variant="primary" onClick={() => openWizard()}>Add your first website</Button>} />
        </Card>
      ) : (
        <div className="grid gap-3">
          {sites.map((s) => {
            const d = domains.find((x) => x.id === s.domainId);
            const busy = act.busy[s.id]; const pp = publishPill(s.publishState);
            return (
              <Card key={s.id} className="p-4">
                <div className="mb-4 flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <button type="button" className="press max-w-full truncate rounded text-left text-[16px] font-semibold hover:text-kelp" onClick={() => go("websites", s.id)}>{s.name}</button>
                    <div className="mt-1 flex items-center gap-2"><StatusPill status={busy ? "starting" : s.status} /><Pill tone={pp.tone} busy={pp.busy}>{pp.text}</Pill>
                      <span className="selectable truncate text-[13px] text-ink3">{s.publicUrl ?? s.localUrl}</span></div>
                  </div>
                  <div className="flex shrink-0 items-center gap-0.5">
                    {s.status === "running"
                      ? <IconButton label={`Stop ${s.name}`} disabled={!!busy} onClick={() => act.stop(s)}><Square size={15} /></IconButton>
                      : <IconButton label={`Start ${s.name}`} disabled={!!busy} onClick={() => act.start(s)}><Play size={15} /></IconButton>}
                    <IconButton label={`Restart ${s.name}`} disabled={!!busy || s.status !== "running"} onClick={() => act.restart(s)}><RotateCw size={15} /></IconButton>
                    <IconButton label={s.publicUrl ? `Open ${s.publicUrl}` : `Preview ${s.name}`} onClick={() => (s.publicUrl ? act.openPublic(s) : act.preview(s))}><ExternalLink size={15} /></IconButton>
                  </div>
                </div>
                <SignalPath site={s} tunnel={tunnelFor(tunnels, s.id)} domain={d?.hostname} />
                {s.statusDetail && <p className={cx("mt-3 text-[13px]", s.status === "error" ? "text-buoy" : "text-ink2")}>{s.statusDetail}</p>}
              </Card>
            );
          })}
        </div>
      )}

      <GuardianCard sites={sites} />
      <TimeMachineCard sites={sites} />

      <div className="grid grid-cols-[1.1fr_1fr] gap-3 max-[1100px]:grid-cols-1">
        <Card className="p-4">
          <h2 className="mb-3 text-[16px] font-semibold">This computer</h2>
          {stats ? (
            <div className="grid gap-3.5">
              <Meter label="Processor" value={stats.cpuPercent} detail={`${stats.cpuPercent.toFixed(0)}% in use`} />
              <Meter label="Memory" value={(stats.ramUsedBytes / stats.ramTotalBytes) * 100} detail={`${fmtBytes(stats.ramUsedBytes)} of ${fmtBytes(stats.ramTotalBytes)}`} />
              <Meter label="Disk" value={(stats.diskUsedBytes / stats.diskTotalBytes) * 100} detail={`${fmtBytes(stats.diskUsedBytes)} of ${fmtBytes(stats.diskTotalBytes)}`} />
              {stats.diskTotalBytes > 0 && stats.diskTotalBytes - stats.diskUsedBytes < 5e9 && (
                <p role="status" className="text-[13px] text-buoy">Only {fmtBytes(stats.diskTotalBytes - stats.diskUsedBytes)} free on the disk Hostify uses. Adding or updating websites can fail until you free some space.</p>
              )}
              <p className="text-[13px] text-ink3">Hostify is using {fmtBytes(stats.hostifyDataBytes)} of storage. Visitor traffic isn't measured in this version.</p>
            </div>
          ) : <div className="grid gap-3.5" role="status" aria-label="Reading system usage"><Skeleton className="h-7" /><Skeleton className="h-7" /><Skeleton className="h-7" /></div>}
        </Card>
        <Card className="p-4">
          <div className="mb-2 flex items-center justify-between"><h2 className="text-[16px] font-semibold">Recent activity</h2>
            <Button variant="ghost" size="sm" onClick={() => go("activity")}>View all activity</Button></div>
          {activity.length === 0 ? <p className="text-ink3">Starting, stopping and publishing show up here.</p> : (
            <ul className="grid gap-2">
              {activity.slice(0, 6).map((a) => (
                <li key={a.id} className="flex gap-2.5">
                  <span className={cx("mt-[7px] h-1.5 w-1.5 shrink-0 rounded-full", a.level === "error" ? "bg-buoy" : a.level === "warn" ? "bg-lantern" : "bg-kelp")} aria-hidden />
                  <div className="min-w-0"><p className="break-words">{a.message}</p><p className="text-[12px] text-ink3">{fmtTime(a.ts)}</p></div>
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>
    </div>
  );
}

function ComponentsBanner() {
  const engine = useStore((s) => s.engine); const installComponents = useStore((s) => s.installComponents); const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ name: string; percent: number }>();
  useEffect(() => {
    let off: (() => void) | undefined; let dead = false;
    listen<{ name: string; percent: number }>("components-progress", setProgress).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, []);
  if (!engine) return null;
  // Older engines report only componentsReady; treat that as "both missing".
  const caddyOk = engine.caddy?.ready ?? engine.componentsReady;
  const tunnelOk = engine.cloudflared?.ready ?? engine.componentsReady;
  if (caddyOk && tunnelOk) return null;
  const names: ComponentName[] = [...(caddyOk ? [] : ["caddy" as const]), ...(tunnelOk ? [] : ["cloudflared" as const])];

  async function install() {
    setBusy(true);
    try { await installComponents(names); toast("ok", names.length === 2 ? "Web server and tunnel connector installed" : `${names[0] === "caddy" ? "Caddy" : "cloudflared"} installed`); }
    catch (e) { toast("error", (e as Error).message); }
    finally { setBusy(false); setProgress(undefined); }
  }
  const problem = (c?: { error?: string }) => (c?.error ? ` (${c.error})` : "");
  const blocking = !caddyOk; // Caddy missing stops all hosting; cloudflared only affects publishing.
  const text = !caddyOk && !tunnelOk
    ? <><strong>Caddy and cloudflared aren't installed.</strong> Caddy hosts your websites on this computer; cloudflared connects them to your domain.</>
    : !caddyOk
      ? <><strong>Caddy, the web server, is missing or can't run{problem(engine.caddy)}.</strong> Websites can't start until it is installed.</>
      : <><strong>cloudflared is missing{problem(engine.cloudflared)}.</strong> Hosting on this computer works. You only need it to publish to a domain.</>;
  return (
    <Card className={cx("flex items-center justify-between gap-4 p-4", blocking ? "border-lantern" : "border-line")}>
      <p>{text} Each is downloaded from its official release and checked against a pinned checksum.</p>
      <Button variant={blocking ? "primary" : "secondary"} busy={busy} onClick={install} className="shrink-0">
        {busy ? `Installing${progress ? ` ${progress.name}: ${progress.percent}%` : ""}` : names.length === 2 ? "Install components" : `Install ${names[0] === "caddy" ? "Caddy" : "cloudflared"}`}
      </Button>
    </Card>
  );
}
