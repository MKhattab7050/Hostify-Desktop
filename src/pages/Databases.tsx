import { useCallback, useEffect, useState } from "react";
import { Copy, Database, Eye, EyeOff, HardDriveDownload, Play, Plus, RefreshCw, Square, Trash2 } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Button, Card, Confirm, Empty, Field, IconButton, Modal, PageHeader, Pill, Skeleton, fmtBytes, fmtTime } from "../components/ui";
import { RuntimeInstall } from "./Runtimes";
import type { DatabaseBackup, DatabaseConnection, ManagedDatabase, MysqlStatus, RuntimeStatus } from "../lib/types";

const NAME_RE = /^[a-z][a-z0-9_]{0,47}$/;
const RESERVED = ["mysql", "sys", "information_schema", "performance_schema"];

function TypedConfirm({ title, name, action, children, onConfirm, onClose }: { title: string; name: string; action: string; children: React.ReactNode; onConfirm: (typed: string) => Promise<void>; onClose: () => void }) {
  const [typed, setTyped] = useState(""); const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  return (
    <Modal title={title} onClose={onClose} footer={<>
      <Button onClick={onClose}>Keep it</Button>
      <Button variant="danger" busy={busy} disabled={typed !== name} onClick={async () => { setBusy(true); setErr(undefined); try { await onConfirm(typed); onClose(); } catch (e) { setErr((e as Error).message); setBusy(false); } }}>{action}</Button>
    </>}>
      <div className="grid gap-3">{children}
        <Field label={`Type ${name} to confirm`} value={typed} spellCheck={false} autoComplete="off" onChange={(e) => setTyped(e.target.value)} />
        {err && <p role="alert" className="selectable text-buoy">{err}</p>}
      </div>
    </Modal>
  );
}

function CreateDialog({ onClose, onCreated }: { onClose: () => void; onCreated: () => void }) {
  const sites = useStore((s) => s.sites).filter((s) => s.kind && s.kind !== "static"); const toast = useStore((s) => s.toast);
  const [name, setName] = useState(""); const [siteId, setSiteId] = useState(""); const [inject, setInject] = useState(true); const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>(); const [conflict, setConflict] = useState(false);
  const valid = NAME_RE.test(name) && !RESERVED.includes(name);
  async function create(e: React.FormEvent, replace = false) {
    e.preventDefault(); setBusy(true); setErr(undefined);
    try {
      const r = await api.createDatabase(name, siteId || undefined, !!siteId && inject, replace || undefined);
      toast("ok", `Database ${name} created${r.restartRequired ? ". Restart the website to use it." : ""}`); onCreated(); onClose();
    } catch (x) { const m = (x as Error).message; setErr(m); setConflict(/already|conflict/i.test(m)); setBusy(false); }
  }
  return (
    <Modal title="Create a database" onClose={onClose}>
      <form className="grid gap-4" onSubmit={create}>
        <Field label="Database name" value={name} spellCheck={false} autoCapitalize="off" placeholder="bakery" onChange={(e) => setName(e.target.value)} onBlur={() => setTouched(true)}
          error={touched && name && !valid ? "Use lowercase letters, numbers and underscores, starting with a letter." : undefined}
          hint="Hostify creates a separate user with access to this database only, with a generated password." />
        <label className="grid gap-1.5"><span className="text-[13px] font-medium">Use it with</span>
          <select value={siteId} onChange={(e) => setSiteId(e.target.value)} className="h-9 rounded-ctl border border-line bg-panel px-2"><option value="">No website yet</option>{sites.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}</select></label>
        {siteId && <label className="flex items-start gap-2.5"><input type="checkbox" className="mt-1" checked={inject} onChange={(e) => setInject(e.target.checked)} />
          <span>Give the website its connection details<span className="block text-[13px] text-ink2">Sets DB_HOST, DB_PORT, DB_NAME, DB_USER and a secret DB_PASSWORD.</span></span></label>}
        {err && <p role="alert" className="selectable text-buoy">{err}</p>}
        <div className="flex justify-end gap-2"><Button onClick={onClose}>Cancel</Button>
          {conflict && <Button variant="danger" busy={busy} onClick={(e) => create(e, true)}>Replace those variables</Button>}
          <Button type="submit" variant="primary" busy={busy} disabled={!valid}>Create database</Button></div>
      </form>
    </Modal>
  );
}

function Details({ db, onChanged, onClose }: { db: ManagedDatabase; onChanged: () => void; onClose: () => void }) {
  const toast = useStore((s) => s.toast);
  const [conn, setConn] = useState<DatabaseConnection>(); const [backups, setBackups] = useState<DatabaseBackup[]>(); const [err, setErr] = useState<string>();
  const [password, setPassword] = useState<string>(); const [test, setTest] = useState<{ ok: boolean; detail: string; elapsedMs: number }>(); const [testing, setTesting] = useState(false);
  const [backingUp, setBackingUp] = useState(false); const [restoring, setRestoring] = useState<string>(); const [rotating, setRotating] = useState(false); const [dropping, setDropping] = useState<string>();
  const user = conn?.user ?? db.users[0];
  const loadBackups = useCallback(() => api.listBackups(db.name).then(setBackups).catch((e) => setErr((e as Error).message)), [db.name]);
  useEffect(() => { api.databaseConnection(db.name).then(setConn).catch((e) => setErr((e as Error).message)); void loadBackups(); }, [db.name, loadBackups]);
  async function runTest() { setTesting(true); setTest(undefined); try { setTest(await api.testConnection(db.name)); } catch (e) { setTest({ ok: false, detail: (e as Error).message, elapsedMs: 0 }); } finally { setTesting(false); } }
  async function backup() { setBackingUp(true); try { await api.backupDatabase(db.name); toast("ok", `Backup of ${db.name} saved`); await loadBackups(); onChanged(); } catch (e) { toast("error", (e as Error).message); } finally { setBackingUp(false); } }
  async function reveal() { if (password !== undefined) { setPassword(undefined); return; } try { setPassword(await api.revealDbPassword(user)); } catch (e) { toast("error", (e as Error).message); } }
  async function copy() { try { await navigator.clipboard.writeText(await api.revealDbPassword(user)); toast("info", "Password copied to the clipboard"); } catch (e) { toast("error", (e as Error).message); } }
  return (
    <Modal title={db.name} wide onClose={onClose}>
      <div className="grid gap-5">
        {err && <p role="alert" className="selectable text-buoy">{err}</p>}
        <section className="grid gap-2"><h3 className="font-semibold">Connection details</h3>
          {conn ? <dl className="grid grid-cols-[100px_minmax(0,1fr)] gap-y-1 text-[13px]">
            {[["Host", conn.host], ["Port", String(conn.port)], ["Database", conn.database], ["User", conn.user]].map(([k, v]) => <div key={k} className="contents"><dt className="text-ink3">{k}</dt><dd className="selectable break-all font-mono">{v}</dd></div>)}
            <dt className="text-ink3">Password</dt><dd className="flex flex-wrap items-center gap-1"><span className="selectable break-all font-mono">{password ?? "••••••••"}</span>
              <IconButton label={password !== undefined ? "Hide password" : "Show password"} onClick={reveal}>{password !== undefined ? <EyeOff size={15} /> : <Eye size={15} />}</IconButton><IconButton label="Copy password" onClick={copy}><Copy size={15} /></IconButton></dd></dl> : !err && <Skeleton className="h-20" />}
          <p className="text-[13px] text-ink2">MySQL only accepts connections from this PC, so only software running here can use these.</p>
          <div className="flex flex-wrap items-center gap-2"><Button size="sm" busy={testing} onClick={runTest}>Test connection</Button><Button size="sm" variant="ghost" onClick={() => setRotating(true)}><RefreshCw size={13} aria-hidden />Change password</Button></div>
          {test && <p role={test.ok ? "status" : "alert"} className={test.ok ? "text-[13px] text-kelp" : "selectable text-[13px] text-buoy"}>{test.ok ? `Connected in ${test.elapsedMs} ms. ` : ""}{test.detail}</p>}
        </section>
        <section className="grid gap-2"><div className="flex items-center justify-between gap-2"><h3 className="font-semibold">Backups</h3><Button size="sm" busy={backingUp} onClick={backup}><HardDriveDownload size={13} aria-hidden />Back up now</Button></div>
          {!backups && !err && <Skeleton className="h-12" />}
          {backups?.length === 0 && <p className="text-[13px] text-ink2">No backups yet. Backups are readable SQL files kept in your Hostify data folder.</p>}
          {backups && backups.length > 0 && <ul className="divide-y divide-line">{backups.map((b) => (
            <li key={b.file} className="flex flex-wrap items-center justify-between gap-2 py-2"><div><div className="font-medium">{fmtTime(b.createdAt)}</div><div className="text-[13px] text-ink2">{fmtBytes(b.sizeBytes)}</div></div>
              <div className="flex gap-1"><Button size="sm" onClick={() => setRestoring(b.file)}>Restore</Button><IconButton label={`Delete backup from ${fmtTime(b.createdAt)}`} onClick={() => setDropping(b.file)}><Trash2 size={15} /></IconButton></div></li>))}</ul>}
        </section>
      </div>
      {restoring && <TypedConfirm title={`Restore ${db.name}?`} name={db.name} action="Restore this backup" onClose={() => setRestoring(undefined)}
        onConfirm={async (typed) => { const r = await api.restoreDatabase(db.name, restoring, typed); toast("ok", r.safetyBackup ? `Restored. The previous contents were saved as ${r.safetyBackup}.` : "Restored."); await loadBackups(); onChanged(); }}>
        <p>This replaces everything in {db.name} with the backup. Hostify saves a backup of the current contents first and stops websites using it during the restore.</p></TypedConfirm>}
      {rotating && <Confirm title={`Change the password for ${user}?`} action="Change password" onClose={() => setRotating(false)}
        onConfirm={async () => { const r = await api.rotateDbPassword(user); setPassword(undefined); toast("ok", r.restartRequired ? "Password changed. Restart the website to use it." : "Password changed."); }}>
        <p>Anything using the old password stops connecting. A linked website gets the new one when it restarts.</p></Confirm>}
      {dropping && <Confirm title="Delete this backup?" action="Delete backup" onClose={() => setDropping(undefined)} onConfirm={async () => { await api.deleteBackup(db.name, dropping); await loadBackups(); }}><p>This can't be undone.</p></Confirm>}
    </Modal>
  );
}

export function Databases() {
  const toast = useStore((s) => s.toast);
  const [status, setStatus] = useState<MysqlStatus>(); const [dbs, setDbs] = useState<ManagedDatabase[]>(); const [runtime, setRuntime] = useState<RuntimeStatus>(); const [err, setErr] = useState<string>();
  const [creating, setCreating] = useState(false); const [open, setOpen] = useState<string>(); const [deleting, setDeleting] = useState<string>(); const [busy, setBusy] = useState<string>();
  const load = useCallback(async () => {
    try {
      const [s, rt] = await Promise.all([api.mysqlStatus(), api.listRuntimes()]);
      setStatus(s); setRuntime(rt.find((r) => r.kind === "mysql")); setErr(undefined);
      setDbs(s.setUp ? await api.listDatabases() : []);
    } catch (e) { setErr((e as Error).message); }
  }, []);
  useEffect(() => { void load(); const t = setInterval(() => { if (!document.hidden) void load(); }, 15000); return () => clearInterval(t); }, [load]);
  async function act(name: string, run: () => Promise<unknown>) { setBusy(name); try { await run(); await load(); } catch (e) { toast("error", (e as Error).message); await load(); } finally { setBusy(undefined); } }
  const detail = dbs?.find((d) => d.name === open);

  return (
    <div className="mx-auto grid max-w-[1040px] gap-5">
      <PageHeader title="Databases" sub="MySQL for your websites. It only accepts connections from this PC.">
        {status?.setUp && <Button variant="primary" disabled={!status.running} onClick={() => setCreating(true)}><Plus size={15} aria-hidden />Create database</Button>}
      </PageHeader>
      {err && <Card className="border-buoy bg-buoysoft p-4"><p role="alert" className="selectable">{err}</p><div className="mt-2"><Button size="sm" onClick={load}>Try again</Button></div></Card>}
      {!status && !err && <Skeleton className="h-32" />}

      {status && !status.setUp && (
        <Card><Empty icon={<Database size={20} />} title={runtime?.ready ? "Set up MySQL" : "Install MySQL first"}
          body={runtime?.ready ? "Hostify creates the database server with a generated administrator password kept in Windows Credential Manager." : "MySQL needs about 1.2 GB of disk space. It stays off until a website needs it."}
          action={runtime?.ready ? <Button variant="primary" busy={busy === "setup"} onClick={() => act("setup", () => api.mysqlSetup(runtime.version))}>Set up MySQL</Button>
            : runtime ? <RuntimeInstall kind="mysql" version={runtime.version} onDone={load} /> : undefined} /></Card>)}

      {status?.setUp && (
        <Card className="grid gap-3 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex flex-wrap items-center gap-2"><h2 className="text-[17px] font-semibold">MySQL {status.version}</h2>
              {status.running ? (status.healthy === false ? <Pill tone="bad">Not responding</Pill> : <Pill tone="ok">Running</Pill>) : <Pill tone="idle">Stopped</Pill>}</div>
            {status.running ? <Button size="sm" busy={busy === "stop"} onClick={() => act("stop", api.mysqlStop)}><Square size={13} aria-hidden />Stop</Button>
              : <Button size="sm" busy={busy === "start"} onClick={() => act("start", api.mysqlStart)}><Play size={13} aria-hidden />Start</Button>}
          </div>
          <div className="grid gap-px overflow-hidden rounded-ctl border border-line bg-line sm:grid-cols-4">
            {[["Port", status.port !== undefined ? String(status.port) : undefined], ["Memory", status.memoryBytes !== undefined ? fmtBytes(status.memoryBytes) : undefined], ["Stored data", status.dataBytes !== undefined ? fmtBytes(status.dataBytes) : undefined], ["Free disk", status.freeDiskBytes !== undefined ? fmtBytes(status.freeDiskBytes) : undefined]].map(([k, v]) => (
              <div key={k} className="bg-panel p-3"><div className="text-[13px] text-ink3">{k}</div><div className="tnum mt-0.5 font-medium">{v ?? "–"}</div></div>))}
          </div>
          {status.lastError && <p role="alert" className="selectable text-[13px] text-buoy">{status.lastError}</p>}
        </Card>)}

      {status?.setUp && dbs?.length === 0 && <Card><Empty icon={<Database size={20} />} title="No databases yet" body="Create one for each website that needs to store data." /></Card>}
      {dbs && dbs.length > 0 && (
        <Card className="overflow-x-auto p-0">
          <table className="w-full min-w-[640px] text-left"><thead><tr className="border-b border-line text-[13px] text-ink3">
            {["Database", "Website", "Size", "Connection", "Last backup", ""].map((h, i) => <th key={i} className="px-4 py-2.5 font-medium">{h}</th>)}</tr></thead>
            <tbody className="divide-y divide-line">{dbs.map((d) => (
              <tr key={d.name}>
                <td className="px-4 py-3"><div className="font-semibold">{d.name}</div><div className="text-[13px] text-ink2">{d.users.join(", ")}</div></td>
                <td className="px-4 py-3">{d.siteName ?? <span className="text-ink3">Not linked</span>}</td>
                <td className="tnum px-4 py-3">{d.sizeBytes !== undefined ? fmtBytes(d.sizeBytes) : "–"}</td>
                <td className="px-4 py-3">{d.connectionOk === null ? <Pill tone="idle">Unknown</Pill> : d.connectionOk ? <Pill tone="ok">Working</Pill> : <Pill tone="bad">Failing</Pill>}</td>
                <td className="px-4 py-3 text-[13px]">{d.lastBackupAt ? fmtTime(d.lastBackupAt) : <span className="text-lantern">Never</span>}</td>
                <td className="px-4 py-3"><div className="flex justify-end gap-1"><Button size="sm" onClick={() => setOpen(d.name)}>Manage</Button><IconButton label={`Delete ${d.name}`} onClick={() => setDeleting(d.name)}><Trash2 size={15} /></IconButton></div></td>
              </tr>))}</tbody></table>
        </Card>)}

      {creating && <CreateDialog onClose={() => setCreating(false)} onCreated={load} />}
      {detail && <Details db={detail} onChanged={load} onClose={() => setOpen(undefined)} />}
      {deleting && <TypedConfirm title={`Delete ${deleting}?`} name={deleting} action={`Delete ${deleting}`} onClose={() => setDeleting(undefined)}
        onConfirm={async (typed) => { await api.deleteDatabase(deleting, typed); toast("ok", `${deleting} deleted`); await load(); }}>
        <p>This permanently deletes the database and its users. Websites using it stop working. Existing backups stay on disk.</p></TypedConfirm>}
    </div>
  );
}
