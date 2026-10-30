import { useEffect, useState } from "react";
import { ExternalLink, FolderOpen, Play, Plus, RefreshCw, RotateCw, Square, Trash2, Rocket, Server, Upload } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Button, Card, Empty, Modal, StatusPill, fmtBytes, fmtTime } from "../components/ui";
import { SignalPath } from "../components/SignalPath";
import { useSiteActions } from "../components/siteActions";
import type { Site, SourceKind } from "../lib/types";

export function Websites() {
  const { sites, siteFocus, openWizard, tunnels, domains } = useStore();
  const [sel, setSel] = useState<string | undefined>(siteFocus);
  useEffect(() => { if (siteFocus) setSel(siteFocus); }, [siteFocus]);
  const site = sites.find((s) => s.id === sel) ?? sites[0];

  if (sites.length === 0)
    return (
      <Card className="mx-auto max-w-[1100px]">
        <Empty icon={<Server size={22} />} title="You have no websites"
          body="Add a folder or ZIP of HTML, CSS and JavaScript. Hostify keeps its own copy, so your original stays untouched."
          action={<Button variant="primary" onClick={() => openWizard()}>Add website</Button>} />
      </Card>
    );

  return (
    <div className="mx-auto grid max-w-[1100px] grid-cols-[280px_1fr] gap-5 max-[1000px]:grid-cols-1">
      <div className="grid content-start gap-2">
        <div className="mb-1 flex items-center justify-between">
          <h1 className="text-2xl font-bold">Websites</h1>
          <Button size="sm" onClick={() => openWizard()}><Plus size={14} aria-hidden />Add</Button>
        </div>
        {sites.map((s) => (
          <button key={s.id} onClick={() => setSel(s.id)} aria-current={site?.id === s.id}
            className={`rounded-card border p-3 text-left transition-colors ${site?.id === s.id ? "border-kelp bg-kelpsoft" : "border-line bg-panel hover:bg-panel2"}`}>
            <div className="truncate font-semibold">{s.name}</div>
            <div className="mt-1.5 flex items-center justify-between"><StatusPill status={s.status} /><span className="text-[12px] text-ink3">{fmtBytes(s.sizeBytes)}</span></div>
          </button>
        ))}
      </div>
      {site && <Detail key={site.id} site={site} tunnel={tunnels[0]} domain={domains.find((d) => d.id === site.domainId)?.hostname} />}
    </div>
  );
}

function Detail({ site, tunnel, domain }: { site: Site; tunnel?: ReturnType<typeof useStore.getState>["tunnels"][number]; domain?: string }) {
  const act = useSiteActions();
  const { openWizard, toast, refresh } = useStore();
  const busy = act.busy[site.id];
  const [logs, setLogs] = useState<string[]>([]);
  const [removing, setRemoving] = useState(false);
  const [updating, setUpdating] = useState(false);

  useEffect(() => {
    let live = true;
    const load = () => api.siteLogs(site.id, 120).then((l) => live && setLogs(l)).catch(() => {});
    void load(); const t = setInterval(load, 4000);
    return () => { live = false; clearInterval(t); };
  }, [site.id]);

  return (
    <div className="grid content-start gap-4">
      <Card className="p-5">
        <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="truncate text-2xl font-bold">{site.name}</h2>
            <div className="mt-1.5 flex items-center gap-3"><StatusPill status={busy ? "starting" : site.status} />
              <span className="text-[13px] text-ink3">Updated {fmtTime(site.updatedAt)}</span></div>
          </div>
          <div className="flex flex-wrap gap-2">
            {site.status === "running"
              ? <Button busy={busy === "stop"} disabled={!!busy} onClick={() => act.stop(site)}><Square size={14} aria-hidden />Stop</Button>
              : <Button busy={busy === "start"} disabled={!!busy} onClick={() => act.start(site)}><Play size={14} aria-hidden />Start</Button>}
            <Button busy={busy === "restart"} disabled={!!busy || site.status !== "running"} onClick={() => act.restart(site)}><RotateCw size={14} aria-hidden />Restart</Button>
            <Button busy={busy === "preview"} disabled={!!busy} onClick={() => act.preview(site)}><FolderOpen size={14} aria-hidden />Local preview</Button>
          </div>
        </div>
        <SignalPath site={site} tunnel={tunnel} domain={domain} />
        {site.statusDetail && <p className="mt-3 text-[13px] text-ink2">{site.statusDetail}</p>}
      </Card>

      <Card className="grid gap-px overflow-hidden bg-line p-0 md:grid-cols-3">
        {[["Local address", site.localUrl], ["Public address", site.publicUrl ?? "Not published"], ["Files", `${site.fileCount} files, ${fmtBytes(site.sizeBytes)}`]].map(([k, v]) => (
          <div key={k} className="bg-panel p-4"><div className="text-[13px] text-ink3">{k}</div><div className="selectable mt-0.5 break-all font-medium">{v}</div></div>
        ))}
      </Card>

      <div className="flex flex-wrap gap-2">
        {site.publicUrl
          ? <Button onClick={() => act.openPublic(site)}><ExternalLink size={14} aria-hidden />Open public site</Button>
          : <Button variant="primary" onClick={() => openWizard(site.id)}><Rocket size={14} aria-hidden />Publish this website</Button>}
        <Button onClick={() => setUpdating(true)}><Upload size={14} aria-hidden />Update files</Button>
        <Button variant="danger" onClick={() => setRemoving(true)} className="ml-auto"><Trash2 size={14} aria-hidden />Remove website</Button>
      </div>

      <Card className="p-0">
        <div className="flex items-center justify-between border-b border-line px-4 py-3"><h3 className="font-semibold">Server log</h3>
          <Button variant="ghost" size="sm" onClick={() => api.siteLogs(site.id, 120).then(setLogs)}><RefreshCw size={13} aria-hidden />Refresh</Button></div>
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all p-4 font-mono text-[12px] leading-5 text-ink2">{logs.length ? logs.join("\n") : "No log lines yet."}</pre>
      </Card>

      {updating && <UpdateFiles site={site} onClose={() => setUpdating(false)} />}
      {removing && (
        <Modal title={`Remove ${site.name}?`} onClose={() => setRemoving(false)} footer={<RemoveFooter site={site} onClose={() => setRemoving(false)} />}>
          <p>This stops the website{site.publicUrl ? ", removes the DNS record and tunnel route Hostify created for it," : ""} and deletes it from Hostify.</p>
          <p className="mt-2 text-ink2">Your original folder is never touched. Records in Cloudflare that Hostify did not create are left alone.</p>
        </Modal>
      )}
    </div>
  );

  function RemoveFooter({ site, onClose }: { site: Site; onClose: () => void }) {
    const [deleteFiles, setDeleteFiles] = useState(true);
    const [working, setWorking] = useState(false);
    return (<>
      <label className="mr-auto flex items-center gap-2 text-[13px]"><input type="checkbox" checked={deleteFiles} onChange={(e) => setDeleteFiles(e.target.checked)} />Also delete Hostify's copy of the files</label>
      <Button onClick={onClose}>Keep website</Button>
      <Button variant="danger" busy={working} onClick={async () => {
        setWorking(true);
        try {
          if (site.publicUrl) await api.unpublishSite(site.id);
          await api.removeSite(site.id, deleteFiles); toast("ok", `${site.name} removed`); onClose(); await refresh();
        } catch (e) { toast("error", (e as Error).message); setWorking(false); }
      }}>Remove {site.name}</Button>
    </>);
  }
}

function UpdateFiles({ site, onClose }: { site: Site; onClose: () => void }) {
  const { toast, refresh } = useStore();
  const [busy, setBusy] = useState<SourceKind | undefined>();
  async function pick(kind: SourceKind) {
    setBusy(kind);
    try {
      const path = await api.pickPath(kind); if (!path) return;
      const rep = await api.validateSource(path, kind);
      if (!rep.ok) { toast("error", rep.errors[0] ?? "Those files can't be hosted."); return; }
      await api.updateSiteFiles(site.id, path, kind); await refresh();
      toast("ok", "Files updated. The previous version is kept for rollback."); onClose();
    } catch (e) { toast("error", (e as Error).message); } finally { setBusy(undefined); }
  }
  return (
    <Modal title="Update website files" onClose={onClose}>
      <p className="mb-4 text-ink2">Hostify swaps in the new files in one step. If anything fails, the current version keeps running.</p>
      <div className="grid grid-cols-2 gap-3">
        <Button busy={busy === "directory"} onClick={() => pick("directory")}><FolderOpen size={15} aria-hidden />Choose folder</Button>
        <Button busy={busy === "zip"} onClick={() => pick("zip")}><Upload size={15} aria-hidden />Choose ZIP</Button>
      </div>
    </Modal>
  );
}
