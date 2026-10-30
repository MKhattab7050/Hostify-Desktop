import { useEffect, useState } from "react";
import { AlertTriangle, Check, CheckCircle2, ExternalLink, FileArchive, FolderOpen, Globe2, Loader2 } from "lucide-react";
import { useStore } from "../store";
import { api, isTauri, onFileDrop } from "../lib/api";
import { isActive, holdsResources } from "../lib/publish";
import { Button, Field, Modal, cx, fmtBytes, fmtFiles } from "./ui";
import { PublishProgress } from "./PublishProgress";
import { ConnectCloudflare } from "../pages/Domains";
import type { DnsPlan, DomainCheck, Site, SourceKind, SourceReport } from "../lib/types";

const HOST_RE = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const STEPS = ["Add website", "Preview", "Choose domain", "Publish"];
const cleanHost = (v: string) => v.trim().toLowerCase().replace(/^https?:\/\//, "").replace(/\/.*$/, "");

function Stepper({ step }: { step: number }) {
  return (
    <ol className="mb-5 flex items-center gap-2" aria-label="Progress">
      {STEPS.map((s, i) => (
        <li key={s} className={cx("flex items-center gap-2", i < STEPS.length - 1 && "flex-1")} aria-current={i === step ? "step" : undefined}>
          <span className={cx("grid h-6 w-6 shrink-0 place-items-center rounded-full text-[12px] font-semibold transition-colors",
            i < step ? "bg-kelp text-kelpink" : i === step ? "border-2 border-kelp text-kelp" : "border border-line text-ink3")}>
            {i < step ? <Check size={13} strokeWidth={3} aria-label="Done" /> : i + 1}</span>
          <span className={cx("whitespace-nowrap text-[13px]", i === step ? "font-semibold" : "text-ink3")}>{s}</span>
          {i < STEPS.length - 1 && <span className={cx("h-px min-w-3 flex-1 transition-colors", i < step ? "bg-kelp" : "bg-line")} />}
        </li>
      ))}
    </ol>
  );
}

export function PublishWizard() {
  const closeWizard = useStore((s) => s.closeWizard); const refresh = useStore((s) => s.refresh); const toast = useStore((s) => s.toast);
  const wizardSiteId = useStore((s) => s.wizardSiteId);
  const existing = useStore((s) => s.sites.find((x) => x.id === wizardSiteId));
  const [step, setStep] = useState(existing ? 2 : 0);
  const [site, setSite] = useState<Site | undefined>(existing);
  const [hostname, setHostname] = useState(existing?.publishHostname ?? "");

  return (
    <Modal title={step === 0 ? "Add a website" : site ? `Publish ${site.name}` : "Publish website"} wide onClose={closeWizard}>
      <Stepper step={step} />
      {step === 0 && <AddStep onAdded={async (s) => { setSite(s); await refresh(); setStep(1); }} />}
      {step === 1 && site && <PreviewStep site={site} onNext={() => setStep(2)} onLocalOnly={() => { toast("ok", `${site.name} is running on this computer`); closeWizard(); }} />}
      {step === 2 && site && <DomainStep hostname={hostname} setHostname={setHostname} onBack={existing ? undefined : () => setStep(1)} onNext={() => setStep(3)} />}
      {step === 3 && site && <PublishStep site={site} hostname={hostname} onDone={closeWizard} onBack={() => setStep(2)} />}
    </Modal>
  );
}

function AddStep({ onAdded }: { onAdded: (s: Site) => void }) {
  const toast = useStore((s) => s.toast);
  const [path, setPath] = useState<string>(); const [kind, setKind] = useState<SourceKind>("directory");
  const [name, setName] = useState(""); const [rep, setRep] = useState<SourceReport>();
  const [busy, setBusy] = useState(false); const [over, setOver] = useState(false);

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
    try { onAdded(await api.addSite(name.trim(), path, kind)); } catch (e) { toast("error", (e as Error).message); setBusy(false); }
  }

  return (
    <div className="grid gap-4">
      <div className={cx("grid place-items-center gap-3 rounded-card border-2 border-dashed px-6 py-8 text-center transition-colors", over ? "border-kelp bg-kelpsoft" : "border-line")}>
        <p className="selectable break-all text-[15px] font-semibold">{path ?? "Drop a ZIP or a folder here"}</p>
        <p className="max-w-md text-ink2">HTML, CSS, JavaScript, images and fonts. Hostify serves the files as they are and never runs them on this computer.</p>
        <div className="flex gap-2"><Button onClick={() => pick("directory")}><FolderOpen size={15} aria-hidden />Choose folder</Button><Button onClick={() => pick("zip")}><FileArchive size={15} aria-hidden />Choose ZIP</Button></div>
        {!isTauri && <p className="text-[12px] text-lantern">Design preview: file dialogs return sample paths.</p>}
      </div>
      {busy && !rep && <p className="flex items-center gap-2 text-ink2" role="status"><Loader2 size={15} className="spin" aria-hidden />Checking files</p>}
      {rep && (
        <div className={cx("rise rounded-card border p-3.5", rep.ok ? "border-line" : "border-buoy bg-buoysoft")} role={rep.ok ? "status" : "alert"}>
          {rep.ok ? <p className="flex items-center gap-2 font-medium"><CheckCircle2 size={16} className="text-kelp" aria-hidden />{fmtFiles(rep.fileCount)}, {fmtBytes(rep.sizeBytes)}{rep.hasIndex ? "" : ". No index.html found"}</p>
            : <p className="font-medium text-buoy">These files can't be hosted</p>}
          {[...rep.errors, ...rep.warnings].map((m, i) => (
            <p key={i} className={cx("mt-1.5 flex gap-2", i < rep.errors.length ? "text-buoy" : "text-lantern")}><AlertTriangle size={15} className="mt-0.5 shrink-0" aria-hidden />{m}</p>))}
        </div>
      )}
      <Field label="Website name" value={name} onChange={(e) => setName(e.target.value)} placeholder="My portfolio" maxLength={60} hint="Only you see this name." />
      <div className="flex justify-end"><Button variant="primary" busy={busy} disabled={!path || !rep?.ok || !name.trim()} onClick={add}>Add website</Button></div>
    </div>
  );
}

function PreviewStep({ site, onNext, onLocalOnly }: { site: Site; onNext: () => void; onLocalOnly: () => void }) {
  const toast = useStore((s) => s.toast);
  const [opened, setOpened] = useState(false);
  async function preview() {
    try { const s = site.status === "running" ? site : await api.startSite(site.id); await api.openExternal(s.localUrl); setOpened(true); }
    catch (e) { toast("error", (e as Error).message); }
  }
  return (
    <div className="grid gap-4">
      <div className="flex items-center gap-3"><CheckCircle2 className="shrink-0 text-kelp" aria-hidden /><div className="min-w-0"><p className="font-semibold">{site.name} is on this computer</p>
        <p className="selectable truncate text-ink2">{site.localUrl}</p></div></div>
      <p className="text-ink2">Only this PC can see it so far. Look it over before it goes anywhere.</p>
      <div><Button variant={opened ? "secondary" : "primary"} onClick={preview}><ExternalLink size={15} aria-hidden />Preview in browser</Button></div>
      <div className="flex justify-end gap-2 border-t border-line pt-4"><Button onClick={onLocalOnly}>Keep it local for now</Button><Button variant={opened ? "primary" : "secondary"} onClick={onNext}>Continue to domain</Button></div>
    </div>
  );
}

function DomainStep({ hostname, setHostname, onBack, onNext }: { hostname: string; setHostname: (v: string) => void; onBack?: () => void; onNext: () => void }) {
  const cf = useStore((s) => s.cf);
  const [connect, setConnect] = useState(false);
  const [check, setCheck] = useState<DomainCheck>(); const [err, setErr] = useState<string>(); const [busy, setBusy] = useState(false);
  const [touched, setTouched] = useState(false);
  const host = cleanHost(hostname); const valid = HOST_RE.test(host);

  async function verify() {
    setBusy(true); setErr(undefined); setCheck(undefined);
    try { const c = await api.cfCheckDomain(host); setCheck(c); if (c.problems.length === 0 && c.canManageDns) onNext(); }
    catch (e) { setErr((e as Error).message); } finally { setBusy(false); }
  }

  if (!cf.connected) return (
    <div className="grid gap-4">
      <div className="flex gap-3"><Globe2 className="mt-0.5 shrink-0 text-ink2" size={18} aria-hidden /><div><p className="font-medium">Connect your Cloudflare account</p>
        <p className="mt-0.5 text-ink2">Your domain's DNS needs to be on Cloudflare. A free account works. Hostify uses a limited token and never asks for your password.</p></div></div>
      <div className="flex justify-between">{onBack ? <Button onClick={onBack}>Back</Button> : <span />}<Button variant="primary" onClick={() => setConnect(true)}>Connect Cloudflare</Button></div>
      {connect && <ConnectCloudflare onClose={() => setConnect(false)} />}
    </div>
  );
  return (
    <form className="grid gap-4" onSubmit={(e) => { e.preventDefault(); if (valid && !busy) void verify(); }}>
      <Field label="Domain" value={hostname} onChange={(e) => { setHostname(e.target.value); setCheck(undefined); setErr(undefined); }} onBlur={() => setTouched(true)}
        placeholder="www.example.com" autoCapitalize="off" spellCheck={false}
        error={touched && hostname && !valid ? "Enter a full domain, like example.com or www.example.com." : err}
        hint="Hostify checks that this domain is in your Cloudflare account and that your token can edit its DNS." />
      {check && check.problems.length > 0 && (
        <div className="rounded-card border border-buoy bg-buoysoft p-3.5" role="alert"><p className="font-medium text-buoy">This domain can't be connected yet</p>
          <ul className="mt-1.5 grid gap-1">{check.problems.map((p, i) => <li key={i}>{p}</li>)}</ul></div>)}
      <div className="flex justify-between">{onBack ? <Button onClick={onBack}>Back</Button> : <span />}
        <Button type="submit" variant="primary" busy={busy} disabled={!valid}>Check domain</Button></div>
    </form>
  );
}

function PublishStep({ site, hostname, onDone, onBack }: { site: Site; hostname: string; onDone: () => void; onBack: () => void }) {
  const setDeployment = useStore((s) => s.setDeployment);
  const live = useStore((s) => s.sites.find((x) => x.id === site.id));
  const host = cleanHost(hostname);
  const [plan, setPlan] = useState<DnsPlan>(); const [consent, setConsent] = useState(false);
  // Reopening the wizard during a long DNS wait lands back on the same deployment.
  const resumable = live?.publishDeploymentId && (isActive(live.publishState) || holdsResources(live.publishState)) ? live.publishDeploymentId : undefined;
  const [depId, setDepId] = useState<string | undefined>(resumable);
  const [starting, setStarting] = useState<string>(); const [err, setErr] = useState<string>();
  const dep = useStore((s) => (depId ? s.deployments[depId] : undefined));

  useEffect(() => { if (depId && !dep) api.getDeployment(depId).then(setDeployment).catch((e) => setErr((e as Error).message)); }, [depId, dep, setDeployment]);
  useEffect(() => { if (!depId) api.cfPlanDns(site.id, host).then(setPlan).catch((e) => setErr((e as Error).message)); }, [site.id, host, depId]);

  async function go() {
    setErr(undefined);
    try {
      if (!useStore.getState().engine?.componentsReady) { setStarting("Installing components"); await useStore.getState().installComponents(); }
      setStarting("Starting");
      const d = await api.publishSite(site.id, host, consent); setDeployment(d); setDepId(d.id);
    }
    catch (e) { setErr((e as Error).message); } finally { setStarting(undefined); }
  }

  if (dep) return <div className="rise"><PublishProgress deployment={dep} onDone={onDone} onRepublish={() => { setDepId(undefined); setConsent(false); }} /></div>;
  return (
    <div className="grid gap-4">
      <div className="rounded-card border border-line p-3.5"><div className="text-[13px] text-ink3">Your website</div><div className="font-semibold">{site.name}</div>
        <div className="mt-2 text-[13px] text-ink3">Goes online at</div><div className="selectable break-all font-semibold">https://{host}</div></div>
      {plan ? (
        <div className={cx("rise rounded-card border p-3.5", plan.action === "conflict" ? "border-buoy bg-buoysoft" : "border-line")} role={plan.action === "conflict" ? "alert" : undefined}>
          <p className="font-medium">{plan.action === "conflict" ? "A DNS record already exists for this domain" : "One DNS change in your Cloudflare account"}</p>
          <p className="mt-1 text-ink2">{plan.message}</p>
          {plan.existing && <p className="selectable mt-1.5 font-mono text-[12px]">{plan.existing.type}: {plan.existing.content}</p>}
          {plan.action === "conflict" && <p className="mt-1.5 text-ink2">Hostify won't overwrite it. Remove the record in Cloudflare, or choose another hostname such as www.{host}.</p>}
        </div>) : !err && <p className="flex items-center gap-2 text-ink2" role="status"><Loader2 size={15} className="spin" aria-hidden />Reading your DNS records</p>}
      {plan && plan.action !== "conflict" && (
        <label className="flex items-start gap-3"><input type="checkbox" className="mt-1" checked={consent} onChange={(e) => setConsent(e.target.checked)} />
          <span>Allow Hostify to make this DNS change. Nothing else in your account changes.</span></label>)}
      {err && <p className="selectable text-buoy" role="alert">{err}</p>}
      <div className="flex justify-between"><Button onClick={onBack}>Back</Button>
        <Button variant="primary" busy={!!starting} disabled={!consent || !plan || plan.action === "conflict"} onClick={go}>{starting ?? "Publish website"}</Button></div>
    </div>
  );
}
