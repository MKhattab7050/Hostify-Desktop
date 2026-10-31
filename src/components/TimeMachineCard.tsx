import { useEffect, useState } from "react";
import { Archive, CheckCircle2, RotateCcw, ShieldAlert } from "lucide-react";
import { api } from "../lib/api";
import type { Site, SiteSnapshot } from "../lib/types";
import { Button, Card, Disclosure, Field, fmtBytes, fmtTime } from "./ui";

/** Source-only snapshots. Database and credential recovery remain separate workflows. */
export function TimeMachineCard({ sites }: { sites: Site[] }) {
  const [records, setRecords] = useState<Record<string, SiteSnapshot[]>>({});
  const [busy, setBusy] = useState<string>();
  const [confirm, setConfirm] = useState<{ siteId: string; snapshotId: string; name: string }>();
  const [error, setError] = useState<string>();
  const [success, setSuccess] = useState<string>();

  useEffect(() => {
    let active = true;
    void Promise.all(sites.map(async (s) => ({ id:s.id, snapshots:await api.listSiteSnapshots(s.id) })))
      .then((result) => { if (active) setRecords(Object.fromEntries(result.map((s) => [s.id,s.snapshots]))); })
      .catch((e) => { if (active) setError((e as Error).message); });
    return () => { active = false; };
  }, [sites.map((s) => s.id).join("|")]);

  async function create(site: Site) {
    setError(undefined);setSuccess(undefined);setBusy(site.id);
    try {
      const snapshot=await api.createSiteSnapshot(site.id);
      setRecords((old)=>({...old,[site.id]:[snapshot,...(old[site.id] ?? [])]}));
      setSuccess(`Saved website files snapshot for ${site.name}. Copy it off this computer for disaster recovery.`);
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(undefined); }
  }
  async function verify(site: Site, snapshot: SiteSnapshot) {
    setBusy(site.id);setError(undefined);setSuccess(undefined);
    try {
      const result=await api.verifySiteSnapshot(site.id,snapshot.id);
      if (!result.verified) throw new Error("Snapshot integrity verification failed.");
      setSuccess(`SHA-256 integrity verified for ${site.name} snapshot ${snapshot.id}.`);
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(undefined); }
  }
  async function restore(site: Site, snapshot: SiteSnapshot) {
    if (confirm?.name !== site.name) return;
    setBusy(site.id);setError(undefined);setSuccess(undefined);
    try {
      const outcome=await api.restoreSiteSnapshot(site.id,snapshot.id,confirm.name);
      setSuccess(outcome.requiresBuild
        ? `Snapshot staged for ${site.name}. Review it, explicitly trust and rebuild before activation.`
        : `Files restored for ${site.name}. Start the site when you're ready.`);
      setConfirm(undefined);
    } catch (e) { setError((e as Error).message); }
    finally { setBusy(undefined); }
  }

  return (
    <Card className="p-4">
      <div className="mb-3 flex items-center gap-2">
        <Archive size={18} className="text-kelp" aria-hidden />
        <div>
          <h2 className="text-[16px] font-semibold">Time Machine · Website files</h2>
          <p className="text-[12px] text-ink2">Versioned, checksum-verified local snapshots. MySQL databases, environment secrets and external storage are <strong>not included</strong>.</p>
        </div>
      </div>
      <p className="mb-3 flex items-start gap-2 rounded-ctl border border-lantern bg-lanternsoft p-3 text-[13px]">
        <ShieldAlert size={16} className="mt-0.5 shrink-0" aria-hidden />
        A snapshot stored on this PC cannot protect you if the computer or disk fails. Export independent database backups and copy snapshots to secure off-device storage.
      </p>
      <ul className="grid divide-y divide-line">
        {sites.map((site) => <li key={site.id} className="py-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-medium">{site.name}</span>
            <Button size="sm" busy={busy===site.id} disabled={!!busy} onClick={() => void create(site)}>
              <Archive size={13} />Create snapshot
            </Button>
          </div>
          <Disclosure title={`Snapshots (${records[site.id]?.length ?? 0})`}>
            {!records[site.id]?.length ? <p className="mt-2 text-[12px] text-ink3">No file snapshots yet.</p> :
              <ul className="mt-2 grid gap-2">
                {records[site.id].map((snapshot) => <li key={snapshot.id} className="rounded-ctl border border-line p-3">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <p className="text-[13px] font-medium">{fmtTime(snapshot.createdAt)}</p>
                      <p className="text-[12px] text-ink2">{snapshot.fileCount} files · {fmtBytes(snapshot.archiveBytes)} · {snapshot.siteKind}</p>
                    </div>
                    <div className="flex gap-2">
                      <Button size="sm" disabled={!!busy} onClick={() => void verify(site,snapshot)}><CheckCircle2 size={13} />Verify</Button>
                      <Button size="sm" variant="danger" disabled={!!busy || site.status!=="stopped"}
                        title={site.status!=="stopped" ? "Stop the site before restoring" : undefined}
                        onClick={() => setConfirm({siteId:site.id,snapshotId:snapshot.id,name:""})}>
                        <RotateCcw size={13} />Restore
                      </Button>
                    </div>
                  </div>
                  {confirm?.siteId===site.id && confirm.snapshotId===snapshot.id && <div className="mt-3 grid gap-2 border-t border-line pt-3">
                    <p className="text-[13px] text-buoy">This changes the site's files. Type its exact name to confirm. Database contents are unchanged.</p>
                    <Field label="Exact website name" value={confirm.name}
                      onChange={(e) => setConfirm({...confirm,name:e.target.value})} />
                    <div className="flex gap-2">
                      <Button size="sm" onClick={() => setConfirm(undefined)}>Cancel</Button>
                      <Button size="sm" variant="danger" busy={!!busy} disabled={confirm.name!==site.name}
                        onClick={() => void restore(site,snapshot)}>Restore verified snapshot</Button>
                    </div>
                  </div>}
                </li>)}
              </ul>}
          </Disclosure>
        </li>)}
      </ul>
      {error && <p className="mt-2 text-[13px] text-buoy" role="alert">{error}</p>}
      {success && <p className="mt-2 text-[13px] text-kelp" role="status">{success}</p>}
    </Card>
  );
}
