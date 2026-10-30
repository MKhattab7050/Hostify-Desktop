import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Circle, FileArchive, FolderOpen, Loader2, XCircle } from "lucide-react";
import { useStore } from "../store";
import { api, isTauri, onFileDrop } from "../lib/api";
import { Button, Field, Modal, cx, fmtBytes } from "./ui";
import { ConnectCloudflare } from "../pages/Domains";
import type { Deployment, DnsPlan, DomainCheck, Site, SourceKind, SourceReport } from "../lib/types";

const HOST_RE = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const STEPS = ["Add website", "Connect domain", "Publish"];

function Stepper({ step }: { step: number }) {
  return (
    <ol className="mb-6 flex items-center gap-2" aria-label="Progress">
      {STEPS.map((s, i) => (
        <li key={s} className="flex flex-1 items-center gap-2" aria-current={i === step ? "step" : undefined}>
          <span className={cx("grid h-7 w-7 shrink-0 place-items-center rounded-full text-[13px] font-semibold",
            i < step ? "bg-kelp text-kelpink" : i === step ? "border-2 border-kelp text-kelp" : "border border-line text-ink3")}>
            {i < step ? "✓" : i + 1}</span>
          <span className={cx("text-[13px]", i === step ? "font-semibold" : "text-ink3")}>{s}</span>
          {i < STEPS.length - 1 && <span className="h-px flex-1 bg-line" />}
        </li>
      ))}
    </ol>
  );
}

export function PublishWizard() {
  const { wizardSiteId, closeWizard, sites, refresh, toast } = useStore();
  const existing = sites.find((s) => s.id === wizardSiteId);
  const [step, setStep] = useState(existing ? 1 : 0);
  const [site, setSite] = useState<Site | undefined>(existing);
  const [hostname, setHostname] = useState("");

  return (
    <Modal title={step === 0 ? "Add a website" : site ? `Publish ${site.name}` : "Publish website"} wide onClose={() => {
      closeWizard();
    }}>
      <Stepper step={step} />
      {step === 0 && <AddStep onAdded={async (s) => { setSite(s); await refresh(); setStep(1); }} onLocalOnly={(s) => { void refresh(); toast("ok", `${s.name} is ready on this computer`); closeWizard(); }} />}
      {step === 1 && site && <DomainStep hostname={hostname} setHostname={setHostname} onBack={existing ? undefined : () => setStep(0)} onNext={() => setStep(2)} />}
      {step === 2 && site && <PublishStep site={site} hostname={hostname} onDone={closeWizard} onBack={() => setStep(1)} />}
    </Modal>
  );
}

/* ---------- Step 1 ---------- */
function AddStep({ onAdded, onLocalOnly }: { onAdded: (s: Site) => void; onLocalOnly: (s: Site) => void }) {
  const { toast } = useStore();
  const [path, setPath] = useState<string>(); const [kind, setKind] = useState<SourceKind>("directory");
  const [name, setName] = useState(""); const [rep, setRep] = useState<SourceReport>();
  const [busy, setBusy] = useState(false); const [over, setOver] = useState(false);
  const [added, setAdded] = useState<Site>();

  async function choose(p: string, k: SourceKind) {
    setPath(p); setKind(k); setRep(undefined); setBusy(true);
    const base = p.split(/[\\/]/).filter(Boolean).pop()?.replace(/\.zip$/i, "") ?? "";
    setName((n) => n || base);
    try { setRep(await api.validateSource(p, k)); } catch (e) { setRep({ ok: false, fileCount: 0, sizeBytes: 0, hasIndex: false, warnings: [], errors: [(e as Error).message] }); }
    finally { setBusy(false); }
  }
  useEffect(() => {
    let off: (() => void) | undefined; let dead = false;
    onFileDrop((o, paths) => { setOver(o); const p = paths[0]; if (p) void choose(p, /\.zip$/i.test(p) ? "zip" : "directory"); }).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, []);

  async function pick(k: SourceKind) { try { const p = await api.pickPath(k); if (p) await choose(p, k); } catch (e) { toast("error", (e as Error).message); } }
  async function add() {
    if (!path) return; setBusy(true);
    try { setAdded(await api.addSite(name.trim(), path, kind)); } catch (e) { toast("error", (e as Error).message); } finally { setBusy(false); }
  }

  if (added) return <Added site={added} onNext={() => onAdded(added)} onLocalOnly={() => onLocalOnly(added)} />;
  return (
    <div className="grid gap-5">
      <div className={cx("grid place-items-center gap-3 rounded-card border-2 border-dashed px-6 py-9 text-center transition-colors", over ? "border-kelp bg-kelpsoft" : "border-line")}>
        <p className="text-lg font-semibold">{path ? path : "Drop a ZIP or a folder here"}</p>
        <p className="text-ink2">Plain HTML, CSS, JavaScript, images and fonts. Hostify never runs the files on your computer.</p>
        <div className="flex gap-2"><Button onClick={() => pick("directory")}><FolderOpen size={15} aria-hidden />Choose folder</Button><Button onClick={() => pick("zip")}><FileArchive size={15} aria-hidden />Choose ZIP</Button></div>
        {!isTauri && <p className="text-[12px] text-lantern">Design preview: file dialogs return sample paths.</p>}
      </div>

      {busy && !rep && <p className="flex items-center gap-2 text-ink2"><Loader2 size={15} className="spin" />Checking files…</p>}
      {rep && (
        <div className={cx("rounded-card border p-4", rep.ok ? "border-line" : "border-buoy bg-buoysoft")} role={rep.ok ? "status" : "alert"}>
          {rep.ok ? <p className="flex items-center gap-2 font-medium"><CheckCircle2 size={16} className="text-kelp" />{rep.fileCount} files · {fmtBytes(rep.sizeBytes)}{rep.hasIndex ? "" : " · no index.html found"}</p>
            : <p className="font-medium text-buoy">These files can't be hosted</p>}
          {[...rep.errors, ...rep.warnings].map((m, i) => (
            <p key={i} className={cx("mt-1.5 flex gap-2", i < rep.errors.length ? "text-buoy" : "text-lantern")}><AlertTriangle size={15} className="mt-0.5 shrink-0" />{m}</p>))}
        </div>
      )}
      <Field label="Website name" value={name} onChange={(e) => setName(e.target.value)} placeholder="My portfolio" maxLength={60} hint="Only you see this name." />
      <div className="flex justify-end"><Button variant="primary" busy={busy} disabled={!path || !rep?.ok || !name.trim()} onClick={add}>Add website</Button></div>
    </div>
  );
}

function Added({ site, onNext, onLocalOnly }: { site: Site; onNext: () => void; onLocalOnly: () => void }) {
  return (
    <div className="grid gap-4">
      <div className="flex items-center gap-3"><CheckCircle2 className="text-kelp" /><div><p className="font-semibold">{site.name} is running on this computer</p>
        <p className="selectable text-ink2">{site.localUrl}</p></div></div>
      <div className="flex flex-wrap gap-2"><Button onClick={() => api.openExternal(site.localUrl)}>Preview website</Button></div>
      <p className="text-ink2">Happy with the preview? Put it on your own domain, or keep it on this computer for now.</p>
      <div className="flex justify-end gap-2"><Button onClick={onLocalOnly}>Keep local only</Button><Button variant="primary" onClick={onNext}>Continue to domain</Button></div>
    </div>
  );
}

/* ---------- Step 2 ---------- */
function DomainStep({ hostname, setHostname, onBack, onNext }: { hostname: string; setHostname: (v: string) => void; onBack?: () => void; onNext: () => void }) {
  const { cf } = useStore();
  const [connect, setConnect] = useState(false);
  const [check, setCheck] = useState<DomainCheck>(); const [err, setErr] = useState<string>(); const [busy, setBusy] = useState(false);
  const host = hostname.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
  const valid = HOST_RE.test(host);

  async function verify() {
    setBusy(true); setErr(undefined); setCheck(undefined);
    try { const c = await api.cfCheckDomain(host); setCheck(c); if (c.problems.length === 0 && c.canManageDns) onNext(); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }

  if (!cf.connected) return (
    <div className="grid gap-4">
      <p>To use your own domain, connect your Cloudflare account. Your domain's DNS must already be on Cloudflare.</p>
      <div className="flex justify-between"><span>{onBack && <Button onClick={onBack}>Back</Button>}</span><Button variant="primary" onClick={() => setConnect(true)}>Connect Cloudflare</Button></div>
      {connect && <ConnectCloudflare onClose={() => setConnect(false)} />}
    </div>
  );
  return (
    <div className="grid gap-4">
      <Field label="Domain" value={hostname} onChange={(e) => { setHostname(e.target.value); setCheck(undefined); setErr(undefined); }}
        placeholder="www.example.com" autoCapitalize="off" spellCheck={false}
        error={hostname && !valid ? "Enter a full domain like example.com or www.example.com." : err}
        hint="Hostify checks that this domain is in your Cloudflare account and that your token can edit its DNS." />
      {check && check.problems.length > 0 && (
        <div className="rounded-card border border-buoy bg-buoysoft p-4" role="alert"><p className="font-medium text-buoy">This domain can't be connected yet</p>
          <ul className="mt-1.5 grid gap-1">{check.problems.map((p, i) => <li key={i}>{p}</li>)}</ul></div>)}
      <div className="flex justify-between"><span>{onBack && <Button onClick={onBack}>Back</Button>}</span>
        <Button variant="primary" busy={busy} disabled={!valid} onClick={verify}>Check domain</Button></div>
    </div>
  );
}

/* ---------- Step 3 ---------- */
function PublishStep({ site, hostname, onDone, onBack }: { site: Site; hostname: string; onDone: () => void; onBack: () => void }) {
  const { deployments, setDeployment, refresh, toast } = useStore();
  const host = hostname.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");
  const [plan, setPlan] = useState<DnsPlan>(); const [consent, setConsent] = useState(false);
  const [depId, setDepId] = useState<string>(); const [starting, setStarting] = useState(false); const [err, setErr] = useState<string>();
  const dep: Deployment | undefined = depId ? deployments[depId] : undefined;

  useEffect(() => { api.cfPlanDns(site.id, host).then(setPlan).catch((e) => setErr((e as Error).message)); }, [site.id, host]);

  // Fallback polling in case an event is missed; also drives the browser-only design preview.
  useEffect(() => {
    if (!depId || dep?.state !== "running") return;
    const t = setInterval(async () => {
      if (!isTauri) {
        const cur = useStore.getState().deployments[depId]; const i = cur.steps.findIndex((s) => s.state !== "done");
        if (i === -1) return;
        const steps = cur.steps.map((s, j) => (j < i + 1 ? { ...s, state: "done" as const } : j === i + 1 ? { ...s, state: "running" as const } : s));
        setDeployment({ ...cur, steps, state: i + 1 >= steps.length ? "live" : "running" });
      } else { try { setDeployment(await api.getDeployment(depId)); } catch { /* keep last known state */ } }
    }, isTauri ? 1500 : 700);
    return () => clearInterval(t);
  }, [depId, dep?.state]);

  async function go() {
    setStarting(true); setErr(undefined);
    try {
      if (!useStore.getState().engine?.componentsReady) await useStore.getState().installComponents();
      const d = await api.publishSite(site.id, host, consent); setDeployment(d); setDepId(d.id);
    }
    catch (e) { setErr((e as Error).message); } finally { setStarting(false); }
  }

  if (!dep) return (
    <div className="grid gap-4">
      <div className="rounded-card border border-line p-4"><div className="text-[13px] text-ink3">Your website</div><div className="font-semibold">{site.name}</div>
        <div className="mt-2 text-[13px] text-ink3">Will be available at</div><div className="selectable font-semibold">https://{host}</div></div>
      {plan && (
        <div className={cx("rounded-card border p-4", plan.action === "conflict" ? "border-buoy bg-buoysoft" : "border-line")} role={plan.action === "conflict" ? "alert" : undefined}>
          <p className="font-medium">{plan.action === "conflict" ? "A DNS record already exists" : "DNS change"}</p>
          <p className="mt-1 text-ink2">{plan.message}</p>
          {plan.existing && <p className="selectable mt-1.5 font-mono text-[12px]">{plan.existing.type} → {plan.existing.content}</p>}
        </div>)}
      {plan?.action !== "conflict" && (
        <label className="flex items-start gap-3"><input type="checkbox" className="mt-1" checked={consent} onChange={(e) => setConsent(e.target.checked)} />
          <span>I allow Hostify to add the DNS record shown above to my Cloudflare account. Hostify changes nothing else.</span></label>)}
      {err && <p className="text-buoy" role="alert">{err}</p>}
      <div className="flex justify-between"><Button onClick={onBack}>Back</Button>
        <Button variant="primary" busy={starting} disabled={!consent || !plan || plan.action === "conflict"} onClick={go}>Publish website</Button></div>
    </div>
  );

  const failed = dep.state === "failed" || dep.state === "rolled_back"; const live = dep.state === "live";
  return (
    <div className="grid gap-4" aria-live="polite">
      <ul className="grid gap-1">
        {dep.steps.map((s) => (
          <li key={s.key} className="flex items-start gap-3 rounded-ctl px-2 py-2">
            <span className="mt-0.5">{s.state === "done" ? <CheckCircle2 size={18} className="text-kelp" /> : s.state === "running" ? <Loader2 size={18} className="spin text-kelp" />
              : s.state === "failed" ? <XCircle size={18} className="text-buoy" /> : <Circle size={18} className="text-ink3" />}</span>
            <div><div className={cx(s.state === "pending" && "text-ink3", s.state === "failed" && "font-medium text-buoy")}>{s.label}</div>
              {s.detail && <div className="text-[13px] text-ink2">{s.detail}</div>}</div>
          </li>))}
      </ul>
      {failed && <div className="rounded-card border border-buoy bg-buoysoft p-4" role="alert"><p className="font-medium text-buoy">{dep.state === "rolled_back" ? "Publishing failed and was rolled back" : "Publishing failed"}</p>
        <p className="mt-1">{dep.error ?? "Something went wrong."}</p><p className="mt-1.5 text-ink2">Nothing that was already online has changed. Fix the problem above and publish again.</p></div>}
      {live && <div className="rounded-card border border-kelp bg-kelpsoft p-4"><p className="font-semibold text-kelp">{site.name} is live</p>
        <p className="selectable mt-0.5">https://{host}</p></div>}
      <div className="flex justify-end gap-2">
        {failed && <Button onClick={() => { setDepId(undefined); }}>Try again</Button>}
        {live && <Button onClick={() => api.openExternal(`https://${host}`)}>Open website</Button>}
        <Button variant={live ? "primary" : "secondary"} disabled={dep.state === "running"} onClick={async () => { await refresh(); if (live) toast("ok", "Published"); onDone(); }}>{dep.state === "running" ? "Publishing…" : "Done"}</Button>
      </div>
    </div>
  );
}
