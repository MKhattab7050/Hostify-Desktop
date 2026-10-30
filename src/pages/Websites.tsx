import { useEffect, useState } from "react";
import { ExternalLink, FolderOpen, Play, Plus, RefreshCw, RotateCw, Square, Trash2, Rocket, Server, Upload } from "lucide-react";
import { tunnelFor, useStore } from "../store";
import { api } from "../lib/api";
import { holdsResources, isActive, publishPill } from "../lib/publish";
import { Button, Card, Confirm, Disclosure, Empty, Modal, PageHeader, Pill, Skeleton, StatusPill, fmtBytes, fmtTime } from "../components/ui";
import { SignalPath } from "../components/SignalPath";
import { PublishProgress } from "../components/PublishProgress";
import { useSiteActions } from "../components/siteActions";
import type { Site, SourceKind } from "../lib/types";

export function Websites() {
  const sites = useStore((s) => s.sites); const siteFocus = useStore((s) => s.siteFocus); const openWizard = useStore((s) => s.openWizard);
  const [sel, setSel] = useState<string | undefined>(siteFocus);
  useEffect(() => { if (siteFocus) setSel(siteFocus); }, [siteFocus]);
  const site = sites.find((s) => s.id === sel) ?? sites[0];

  if (sites.length === 0)
    return (
      <div className="mx-auto grid max-w-[1040px] gap-5"><PageHeader title="Websites" />
        <Card>
          <Empty icon={<Server size={20} />} title="No websites yet"
            body="Add a folder or ZIP of HTML, CSS and JavaScript. Hostify keeps its own copy, so your original stays untouched."
            action={<Button variant="primary" onClick={() => openWizard()}>Add website</Button>} />
        </Card></div>
    );

  return (
    <div className="mx-auto grid max-w-[1040px] grid-cols-[256px_minmax(0,1fr)] gap-5 max-[1000px]:grid-cols-1">
      <div className="grid content-start gap-2">
        <div className="mb-1 flex items-center justify-between">
          <h1 className="text-[26px] font-bold">Websites</h1>
          <Button size="sm" onClick={() => openWizard()}><Plus size={14} aria-hidden />Add website</Button>
        </div>
        {sites.map((s) => {
          const on = site?.id === s.id; const pp = publishPill(s.publishState);
          return (
            <button key={s.id} type="button" onClick={() => setSel(s.id)} aria-current={on || undefined}
              className={`press rounded-card border p-3 text-left ${on ? "border-kelp bg-panel" : "border-line bg-panel hover:border-linestrong"}`}>
              <div className="truncate font-semibold">{s.name}</div>
              <div className="mt-1.5 flex flex-wrap items-center gap-1.5"><StatusPill status={s.status} /><Pill tone={pp.tone} busy={pp.busy}>{pp.text}</Pill></div>
            </button>);
        })}
      </div>
      {site && <Detail key={site.id} site={site} />}
    </div>
  );
}

function Detail({ site }: { site: Site }) {
  const act = useSiteActions();
  const openWizard = useStore((s) => s.openWizard); const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const setDeployment = useStore((s) => s.setDeployment);
  const tunnel = useStore((s) => tunnelFor(s.tunnels, site.id));
  const domain = useStore((s) => s.domains.find((d) => d.id === site.domainId)?.hostname);
  const deployment = useStore((s) => (site.publishDeploymentId ? s.deployments[site.publishDeploymentId] : undefined));
  const busy = act.busy[site.id];
  const [logs, setLogs] = useState<string[]>(); const [logErr, setLogErr] = useState<string>();
  const [removing, setRemoving] = useState(false); const [updating, setUpdating] = useState(false); const [unpublishing, setUnpublishing] = useState(false);
  const holding = holdsResources(site.publishState);
  const showProgress = !!deployment && (isActive(site.publishState) || site.publishState === "recovery_required" || site.publishState === "failed");

  useEffect(() => { if (site.publishDeploymentId && !deployment) api.getDeployment(site.publishDeploymentId).then(setDeployment).catch(() => {}); }, [site.publishDeploymentId, deployment, setDeployment]);
  useEffect(() => {
    let live = true;
    const load = () => { if (document.hidden) return; api.siteLogs(site.id, 120).then((l) => { if (live) { setLogs(l); setLogErr(undefined); } }).catch((e) => live && setLogErr((e as Error).message)); };
    load(); const t = setInterval(load, 5000);
    return () => { live = false; clearInterval(t); };
  }, [site.id]);

  return (
    <div className="grid content-start gap-3">
      <Card className="p-4">
        <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="truncate text-[22px] font-bold">{site.name}</h2>
            <div className="mt-1 flex items-center gap-2.5"><StatusPill status={busy ? "starting" : site.status} />
              <span className="text-[13px] text-ink3">Updated {fmtTime(site.updatedAt)}</span></div>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {site.status === "running"
              ? <Button size="sm" busy={busy === "stop"} disabled={!!busy} onClick={() => act.stop(site)}><Square size={13} aria-hidden />Stop</Button>
              : <Button size="sm" busy={busy === "start"} disabled={!!busy} onClick={() => act.start(site)}><Play size={13} aria-hidden />Start</Button>}
            <Button size="sm" busy={busy === "restart"} disabled={!!busy || site.status !== "running"} onClick={() => act.restart(site)}><RotateCw size={13} aria-hidden />Restart</Button>
            <Button size="sm" busy={busy === "preview"} disabled={!!busy} onClick={() => act.preview(site)}><FolderOpen size={13} aria-hidden />Preview</Button>
          </div>
        </div>
        <SignalPath site={site} tunnel={tunnel} domain={domain} />
        {site.statusDetail && <p className="mt-3 text-[13px] text-ink2">{site.statusDetail}</p>}
      </Card>

      {showProgress && deployment && <Card className="p-4"><h3 className="mb-3 text-[16px] font-semibold">Publishing {deployment.hostname}</h3>
        <PublishProgress deployment={deployment} onRepublish={() => openWizard(site.id)} /></Card>}

      <Card className="grid gap-px overflow-hidden bg-line p-0 md:grid-cols-[1fr_1.5fr_1fr]">
        {[["Local address", site.localUrl], ["Public address", site.publicUrl ?? site.publishHostname ?? "Not published"], ["Files", `${site.fileCount} files, ${fmtBytes(site.sizeBytes)}`]].map(([k, v]) => (
          <div key={k} className="bg-panel p-3.5"><div className="text-[13px] text-ink3">{k}</div><div className="selectable mt-0.5 break-all font-medium">{v}</div></div>
        ))}
      </Card>

      <div className="flex flex-wrap gap-2">
        {site.publicUrl && <Button onClick={() => act.openPublic(site)}><ExternalLink size={14} aria-hidden />Open public site</Button>}
        {!holding && !isActive(site.publishState) && <Button variant="primary" onClick={() => openWizard(site.id)}><Rocket size={14} aria-hidden />Publish this website</Button>}
        {site.publishState === "published" && <Button onClick={() => setUnpublishing(true)}>Unpublish</Button>}
        <Button onClick={() => setUpdating(true)}><Upload size={14} aria-hidden />Update files</Button>
        <Button variant="danger" onClick={() => setRemoving(true)} className="ml-auto"><Trash2 size={14} aria-hidden />Remove website</Button>
      </div>

      <Card className="p-4">
        <Disclosure title="Server log and details">
          <div className="mt-2 grid gap-2">
            <p className="selectable break-all text-[13px] text-ink2">Imported from {site.sourcePath}</p>
            <div className="flex justify-end"><Button variant="ghost" size="sm" onClick={() => api.siteLogs(site.id, 120).then(setLogs).catch((e) => setLogErr((e as Error).message))}><RefreshCw size={13} aria-hidden />Refresh log</Button></div>
            {logErr && <p role="alert" className="text-[13px] text-buoy">{logErr}</p>}
            {logs ? <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-ctl bg-chalk p-3 font-mono text-[12px] leading-5 text-ink2">{logs.length ? logs.join("\n") : "No requests logged yet."}</pre> : <Skeleton className="h-24" />}
          </div>
        </Disclosure>
      </Card>

      {updating && <UpdateFiles site={site} onClose={() => setUpdating(false)} />}
      {unpublishing && (
        <Confirm title={`Unpublish ${site.publishHostname ?? site.name}?`} action={`Unpublish ${site.publishHostname ?? site.name}`} onClose={() => setUnpublishing(false)}
          onConfirm={async () => { await api.unpublishSite(site.id); await refresh(); toast("ok", "Website unpublished. It still runs on this computer."); }}>
          <p>Hostify removes the DNS record and tunnel route it created. The website keeps running on this computer. Records Hostify didn't create are left alone.</p>
        </Confirm>)}
      {removing && <RemoveDialog site={site} holding={holding} onClose={() => setRemoving(false)} />}
    </div>
  );
}

function RemoveDialog({ site, holding, onClose }: { site: Site; holding: boolean; onClose: () => void }) {
  const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const [deleteFiles, setDeleteFiles] = useState(true);
  return (
    <Confirm title={`Remove ${site.name}?`} action={`Remove ${site.name}`} onClose={onClose}
      onConfirm={async () => {
        if (holding) await api.unpublishSite(site.id);
        await api.removeSite(site.id, deleteFiles); toast("ok", `${site.name} removed`); await refresh();
      }}>
      <p>This stops the website{holding ? ", removes the DNS record and tunnel route Hostify created for it," : ""} and deletes it from Hostify.</p>
      <p className="mt-2 text-ink2">Your original folder is never touched. Records in Cloudflare that Hostify didn't create are left alone.</p>
      <label className="mt-3 flex items-center gap-2"><input type="checkbox" checked={deleteFiles} onChange={(e) => setDeleteFiles(e.target.checked)} />Also delete Hostify's copy of the files</label>
    </Confirm>
  );
}

function UpdateFiles({ site, onClose }: { site: Site; onClose: () => void }) {
  const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const [busy, setBusy] = useState<SourceKind | undefined>();
  async function pick(kind: SourceKind) {
    setBusy(kind);
    try {
      const path = await api.pickPath(kind); if (!path) return;
      const rep = await api.validateSource(path, kind);
      if (!rep.ok) { toast("error", rep.errors[0] ?? "Those files can't be hosted."); return; }
      await api.updateSiteFiles(site.id, path, kind); await refresh();
      toast("ok", "Files updated. The previous version is kept."); onClose();
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
