import { ExternalLink, Play, Plus, RotateCw, Square } from "lucide-react";
import { useStore } from "../store";
import { Button, Card, Empty, IconButton, Meter, StatusPill, fmtBytes, fmtTime, cx } from "../components/ui";
import { SignalPath } from "../components/SignalPath";
import { useSiteActions } from "../components/siteActions";
import { Rocket } from "lucide-react";

export function Dashboard() {
  const { sites, stats, tunnels, activity, openWizard, go, engine, domains } = useStore();
  const act = useSiteActions();
  const online = sites.filter((s) => s.status === "running").length;
  const tunnel = tunnels[0];

  return (
    <div className="mx-auto grid max-w-[1100px] gap-6">
      <header className="flex items-end justify-between gap-4">
        <div>
          <h1 className="text-3xl font-bold">{sites.length === 0 ? "Let's put a website online" : `${online} of ${sites.length} ${sites.length === 1 ? "website" : "websites"} online`}</h1>
          <p className="mt-1 text-ink2">
            {tunnel
              ? tunnel.connected ? `Tunnel connected with ${tunnel.connections} connections.` : "Tunnel is disconnected. Hostify is retrying."
              : "No tunnel yet. It is created the first time you publish."}
          </p>
        </div>
        <Button variant="primary" onClick={() => openWizard()}><Plus size={16} aria-hidden />Add website</Button>
      </header>

      {engine && !engine.componentsReady && (
        <Card className="flex items-center justify-between gap-4 border-lantern p-4">
          <p><strong>Hosting components are not installed yet.</strong> Hostify downloads and verifies its web server and tunnel connector before the first publish.</p>
        </Card>
      )}

      {sites.length === 0 ? (
        <Card>
          <Empty icon={<Rocket size={22} />} title="No websites yet"
            body="Pick a folder or drop a ZIP. You can preview it here before it goes anywhere near the internet."
            action={<Button variant="primary" onClick={() => openWizard()}>Add your first website</Button>} />
        </Card>
      ) : (
        <div className="grid gap-4">
          {sites.map((s, i) => {
            const d = domains.find((x) => x.id === s.domainId);
            const busy = act.busy[s.id];
            return (
              <Card key={s.id} className="rise p-5" >
                <div style={{ animationDelay: `${i * 40}ms` }}>
                  <div className="mb-5 flex items-center justify-between gap-3">
                    <div className="min-w-0">
                      <button className="truncate text-left text-lg font-semibold hover:text-kelp" onClick={() => go("websites", s.id)}>{s.name}</button>
                      <div className="mt-1 flex items-center gap-3"><StatusPill status={busy ? "starting" : s.status} />
                        <span className="selectable truncate text-[13px] text-ink3">{s.publicUrl ?? s.localUrl}</span></div>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      {s.status === "running"
                        ? <IconButton label={`Stop ${s.name}`} disabled={!!busy} onClick={() => act.stop(s)}><Square size={16} /></IconButton>
                        : <IconButton label={`Start ${s.name}`} disabled={!!busy} onClick={() => act.start(s)}><Play size={16} /></IconButton>}
                      <IconButton label={`Restart ${s.name}`} disabled={!!busy || s.status !== "running"} onClick={() => act.restart(s)}><RotateCw size={16} /></IconButton>
                      <IconButton label={s.publicUrl ? "Open public site" : "Open local preview"} onClick={() => (s.publicUrl ? act.openPublic(s) : act.preview(s))}><ExternalLink size={16} /></IconButton>
                    </div>
                  </div>
                  <SignalPath site={s} tunnel={tunnel} domain={d?.hostname} />
                  {s.statusDetail && <p className={cx("mt-3 text-[13px]", s.status === "error" ? "text-buoy" : "text-ink2")}>{s.statusDetail}</p>}
                </div>
              </Card>
            );
          })}
        </div>
      )}

      <div className="grid grid-cols-[1.1fr_1fr] gap-4 max-[1100px]:grid-cols-1">
        <Card className="p-5">
          <h2 className="mb-4 text-lg font-semibold">This computer</h2>
          {stats ? (
            <div className="grid gap-4">
              <Meter label="Processor" value={stats.cpuPercent} detail={`${stats.cpuPercent.toFixed(0)}% in use`} />
              <Meter label="Memory" value={(stats.ramUsedBytes / stats.ramTotalBytes) * 100} detail={`${fmtBytes(stats.ramUsedBytes)} of ${fmtBytes(stats.ramTotalBytes)}`} />
              <Meter label="Disk" value={(stats.diskUsedBytes / stats.diskTotalBytes) * 100} detail={`${fmtBytes(stats.diskUsedBytes)} of ${fmtBytes(stats.diskTotalBytes)}`} />
              <p className="text-[13px] text-ink3">Your websites use {fmtBytes(stats.hostifyDataBytes)} of storage. Visitor traffic is not measured in this version.</p>
            </div>
          ) : <p className="text-ink3">Reading system usage…</p>}
        </Card>
        <Card className="p-5">
          <div className="mb-3 flex items-center justify-between"><h2 className="text-lg font-semibold">Recent activity</h2>
            <Button variant="ghost" size="sm" onClick={() => go("activity")}>See all</Button></div>
          {activity.length === 0 ? <p className="text-ink3">Nothing has happened yet.</p> : (
            <ul className="grid gap-2.5">
              {activity.slice(0, 6).map((a) => (
                <li key={a.id} className="flex gap-3">
                  <span className={cx("mt-2 h-1.5 w-1.5 shrink-0 rounded-full", a.level === "error" ? "bg-buoy" : a.level === "warn" ? "bg-lantern" : "bg-kelp")} />
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
