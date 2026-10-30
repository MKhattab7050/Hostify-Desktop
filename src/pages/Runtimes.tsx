import { useCallback, useEffect, useState } from "react";
import { Boxes, Download, Trash2 } from "lucide-react";
import { useStore } from "../store";
import { api, listen } from "../lib/api";
import { Button, Card, Confirm, Empty, Meter, PageHeader, Pill, Skeleton, fmtBytes } from "../components/ui";
import type { RuntimeKind, RuntimeProgress, RuntimeStatus } from "../lib/types";

export const RUNTIME_NAMES: Record<RuntimeKind, string> = { node: "Node.js", php: "PHP", mysql: "MySQL" };
const PHASE: Record<RuntimeProgress["phase"], string> = { download: "Downloading", verify: "Checking the download", extract: "Unpacking", check: "Testing that it runs" };

/** One install at a time per row; progress comes from the engine, never from a timer. */
export function RuntimeInstall({ kind, version, onDone, label = "Install" }: { kind: RuntimeKind; version: string; onDone: () => void; label?: string }) {
  const toast = useStore((s) => s.toast);
  const [busy, setBusy] = useState(false); const [progress, setProgress] = useState<RuntimeProgress>(); const [err, setErr] = useState<string>();
  useEffect(() => {
    let off: (() => void) | undefined; let dead = false;
    listen<RuntimeProgress>("runtime-progress", (p) => { if (p.kind === kind && p.version === version) setProgress(p); }).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, [kind, version]);
  async function install() {
    setBusy(true); setErr(undefined); setProgress(undefined);
    try { await api.installRuntime(kind, version); toast("ok", `${RUNTIME_NAMES[kind]} ${version} installed and tested`); onDone(); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }
  const percent = progress?.total ? (progress.done / progress.total) * 100 : 0;
  return (
    <div className="grid gap-2">
      <div><Button size="sm" busy={busy} onClick={install}><Download size={13} aria-hidden />{label} {RUNTIME_NAMES[kind]} {version}</Button></div>
      {busy && progress && <Meter value={percent} label={PHASE[progress.phase]} detail={progress.total ? `${fmtBytes(progress.done)} of ${fmtBytes(progress.total)}` : fmtBytes(progress.done)} />}
      {err && <p role="alert" className="selectable text-[13px] text-buoy">{err}</p>}
    </div>
  );
}

export function Runtimes() {
  const sites = useStore((s) => s.sites); const toast = useStore((s) => s.toast);
  const [list, setList] = useState<RuntimeStatus[]>(); const [err, setErr] = useState<string>(); const [removing, setRemoving] = useState<RuntimeStatus>();
  const load = useCallback(() => api.listRuntimes().then((l) => { setList(l); setErr(undefined); }).catch((e) => setErr((e as Error).message)), []);
  useEffect(() => { void load(); }, [load]);
  const used = (list ?? []).reduce((n, r) => n + (r.sizeBytes ?? 0), 0);

  return (
    <div className="mx-auto grid max-w-[1040px] gap-5">
      <PageHeader title="Runtimes" sub={list ? `The software your apps run on. Installed copies use ${fmtBytes(used)}.` : "The software your apps run on."} />
      {err && <Card className="border-buoy bg-buoysoft p-4"><p role="alert" className="selectable">{err}</p><div className="mt-2"><Button size="sm" onClick={load}>Try again</Button></div></Card>}
      {!list && !err && <Skeleton className="h-44" />}
      {list && list.length === 0 && <Card><Empty icon={<Boxes size={20} />} title="No runtimes available" body="This version of Hostify doesn't list any runtimes." /></Card>}
      {(["node", "php", "mysql"] as RuntimeKind[]).map((kind) => {
        const rows = (list ?? []).filter((r) => r.kind === kind); if (!rows.length) return null;
        return (
          <Card key={kind} className="p-4">
            <h2 className="mb-1 text-[17px] font-semibold">{RUNTIME_NAMES[kind]}</h2>
            <p className="mb-2 text-[13px] text-ink2">Downloaded from the publisher and checked against a fingerprint built into Hostify. New versions arrive with Hostify updates.</p>
            <ul className="divide-y divide-line">
              {rows.map((r) => {
                const names = r.usedBy.map((id) => sites.find((s) => s.id === id)?.name ?? id);
                return (
                  <li key={r.version} className="flex flex-wrap items-start justify-between gap-3 py-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2"><span className="font-semibold">{r.version}</span>
                        {r.ready ? <Pill tone="ok">Ready</Pill> : r.installed ? <Pill tone="bad">Can't run</Pill> : <Pill tone="idle">Not installed</Pill>}</div>
                      <p className="mt-0.5 text-[13px] text-ink2">{r.sizeBytes !== undefined ? fmtBytes(r.sizeBytes) : "Size appears once installed"}{names.length ? `. Used by ${names.join(", ")}` : ""}</p>
                      {r.error && <p role="alert" className="selectable mt-1 text-[13px] text-buoy">{r.error}</p>}
                    </div>
                    {r.installed
                      ? <Button size="sm" variant="danger" disabled={r.usedBy.length > 0} title={r.usedBy.length ? "Websites still use this version" : undefined} onClick={() => setRemoving(r)}><Trash2 size={13} aria-hidden />Remove</Button>
                      : <RuntimeInstall kind={r.kind} version={r.version} onDone={load} />}
                  </li>
                );
              })}
            </ul>
          </Card>
        );
      })}
      {removing && (
        <Confirm title={`Remove ${RUNTIME_NAMES[removing.kind]} ${removing.version}?`} action={`Remove ${RUNTIME_NAMES[removing.kind]} ${removing.version}`} onClose={() => setRemoving(undefined)}
          onConfirm={async () => { await api.removeRuntime(removing.kind, removing.version); toast("ok", `${RUNTIME_NAMES[removing.kind]} ${removing.version} removed`); await load(); }}>
          <p>This frees its disk space. You can install it again at any time. Hostify refuses if a website or stored database still needs it.</p>
        </Confirm>)}
    </div>
  );
}
