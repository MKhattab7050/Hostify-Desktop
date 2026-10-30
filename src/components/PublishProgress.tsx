import { useEffect, useState } from "react";
import { AlertTriangle, Check, Clipboard, ExternalLink, Loader2, RefreshCw, X } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { diagnosticsText, isActive, isProvisioning, isVerifying, nextAction, redact, stagesOf, type Stage } from "../lib/publish";
import type { Deployment, VerifyCheck } from "../lib/types";
import { Button, Confirm, Disclosure, cx } from "./ui";

function Countdown({ to }: { to?: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(t); }, []);
  if (!to) return <>Checking now</>;
  const s = Math.max(0, Math.round((+new Date(to) - now) / 1000));
  return <>{s === 0 ? "Checking now" : `Next check in ${s} s`}</>;
}

function StageIcon({ state }: { state: Stage["state"] }) {
  const base = "relative z-10 grid h-7 w-7 shrink-0 place-items-center rounded-full border-2";
  if (state === "done") return <span className={cx(base, "border-kelp bg-kelp text-kelpink")}><Check size={14} strokeWidth={3} aria-hidden /></span>;
  if (state === "failed") return <span className={cx(base, "border-buoy bg-panel text-buoy")}><X size={14} strokeWidth={3} aria-hidden /></span>;
  if (state === "running") return <span className={cx(base, "pulse border-kelp bg-panel text-kelp")}><Loader2 size={14} className="spin" aria-hidden /></span>;
  if (state === "waiting") return <span className={cx(base, "pulse border-lantern bg-panel text-lantern")}><span className="h-2 w-2 rounded-full bg-current" /></span>;
  return <span className={cx(base, "border-line bg-panel text-ink3")} />;
}

const STATE_WORD: Record<Stage["state"], string> = { pending: "not started", running: "in progress", done: "done", failed: "needs attention", waiting: "waiting" };

function Route({ stages, detail }: { stages: Stage[]; detail?: string }) {
  return (
    <ol className="grid">
      {stages.map((s, i) => {
        const open = s.state === "running" || s.state === "waiting";
        const current = open || s.state === "failed";
        return (
          <li key={s.key} className="relative flex gap-3 pb-3 last:pb-0" aria-current={current ? "step" : undefined}>
            {i < stages.length - 1 && <span className="route-wire" data-state={s.state === "done" ? "done" : s.state === "waiting" ? "live" : "idle"} aria-hidden />}
            <StageIcon state={s.state} />
            <div className="min-w-0 pt-0.5">
              <div className={cx("font-medium", s.state === "pending" && "text-ink3", s.state === "failed" && "text-buoy")}>
                {s.label}<span className="sr-only">: {STATE_WORD[s.state]}</span></div>
              {open && <p className="rise mt-0.5 text-[13px] text-ink2">{detail && s.state === "waiting" ? detail : s.explain}</p>}
            </div>
          </li>
        );
      })}
    </ol>
  );
}

function Checks({ checks }: { checks: VerifyCheck[] }) {
  return (
    <ul className="mt-2 grid gap-1.5 rounded-ctl bg-chalk p-3 text-[13px]">
      {checks.map((c) => (
        <li key={c.key} className="flex items-start gap-2">
          <span className={cx("mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full", c.state === "passed" ? "bg-kelp" : c.state === "failed" ? "bg-buoy" : "bg-linestrong")} aria-hidden />
          <span className="min-w-0"><span className="font-medium">{c.label}</span><span className="sr-only">: {c.state}</span>
            {c.detail && <span className="selectable block break-words text-ink2">{c.detail}</span>}</span>
        </li>))}
    </ul>
  );
}

/** Live status for one deployment. Used in the wizard and on a website's page, so leaving never loses it. */
export function PublishProgress({ deployment, onRepublish, onDone }: { deployment: Deployment; onRepublish?: () => void; onDone?: () => void }) {
  const setDeployment = useStore((s) => s.setDeployment); const toast = useStore((s) => s.toast); const refresh = useStore((s) => s.refresh);
  const tunnels = useStore((s) => s.tunnels);
  const d = deployment; const v = d.verification; const host = d.hostname;
  const [busy, setBusy] = useState<"retry" | "cancel">(); const [unpublish, setUnpublish] = useState(false);
  const active = isActive(d.state);

  // Events drive this view. The slow poll only covers an event lost while the pipe reconnected.
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => { api.getDeployment(d.id).then(setDeployment).catch(() => {}); }, 4000);
    return () => clearInterval(t);
  }, [d.id, active, setDeployment]);

  async function act(kind: "retry" | "cancel") {
    setBusy(kind);
    try { setDeployment(await (kind === "retry" ? api.retryVerification(d.id) : api.cancelDeployment(d.id))); await refresh(); }
    catch (e) { toast("error", (e as Error).message); } finally { setBusy(undefined); }
  }
  async function copy() {
    try { await navigator.clipboard.writeText(redact(diagnosticsText(d, tunnels[0]))); toast("ok", "Diagnostics copied"); }
    catch { toast("error", "Windows blocked clipboard access. Select the text under Details and copy it."); }
  }

  const published = d.state === "published"; const stuck = d.state === "recovery_required";
  const failed = d.state === "failed" || d.state === "rolled_back"; const verifying = isVerifying(d.state);

  return (
    <div className="grid gap-4">
      {published && (
        <div className="flex items-center gap-3 rounded-card border border-kelp bg-kelpsoft p-4" role="status">
          <span className="pop grid h-10 w-10 shrink-0 place-items-center rounded-full bg-kelp text-kelpink">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden><path className="check-draw" d="M5 13l4 4L19 7" /></svg></span>
          <div className="min-w-0"><p className="font-display text-lg font-semibold">Your website is on the internet</p>
            <p className="selectable break-all text-ink2">https://{host}</p></div>
        </div>)}
      {published && v?.visibleFromThisPc === false && (
        <p className="rounded-ctl bg-lanternsoft p-3 text-[13px]" role="status">Hostify reached your site through Cloudflare, but this PC still remembers an older DNS answer, so it may not open here for up to 30 minutes. Visitors elsewhere aren't affected and it clears by itself.</p>)}

      {!published && <Route stages={stagesOf(d)} detail={verifying ? v?.message : undefined} />}

      {verifying && <p className="tnum text-[13px] text-ink3" aria-live="off">Attempt {v?.attempt ?? 1}. <Countdown to={v?.nextRetryAt} />. You can close this window: checking continues and your Cloudflare setup stays in place.</p>}

      {stuck && (
        <div className="rounded-card border border-buoy bg-buoysoft p-4" role="alert">
          <p className="flex items-center gap-2 font-semibold"><AlertTriangle size={16} className="text-buoy" aria-hidden />Hostify paused checking your public website</p>
          <p className="mt-1">{v?.message ?? d.error ?? "The public check hasn't succeeded yet."}</p>
          <p className="mt-1.5 text-ink2">{nextAction(v?.errorClass, true) ?? "Check again when you're ready."} Nothing was removed: your DNS record, tunnel and website are still in place.</p>
        </div>)}
      {verifying && v?.errorClass && !["dns_not_visible", "tls_pending"].includes(v.errorClass) && <p className="text-[13px] text-ink2">{nextAction(v.errorClass)}</p>}
      {v?.nameservers?.length && v.errorClass === "zone_not_active" ? (
        <div className="rounded-ctl bg-chalk p-3"><p className="text-[13px] font-medium">Cloudflare nameservers for this domain</p>
          <ul className="selectable mt-1 font-mono text-[12px]">{v.nameservers.map((n) => <li key={n}>{n}</li>)}</ul></div>) : null}

      {failed && (
        <div className="rounded-card border border-buoy bg-buoysoft p-4" role="alert">
          <p className="font-semibold">{d.state === "rolled_back" ? "Publishing stopped and was undone" : "Publishing failed"}</p>
          <p className="selectable mt-1 break-words">{d.error ?? "Hostify didn't record a reason. Copy the diagnostics below."}</p>
          <p className="mt-1.5 text-ink2">Hostify removed only what this attempt created. Anything already online is unchanged.</p>
        </div>)}

      {(d.verification || d.steps.some((s) => s.detail)) && (
        <Disclosure title="Details">
          {v && v.checks.length > 0 && <Checks checks={v.checks} />}
          <div className="mt-2 flex items-center gap-2">
            {v?.lastCheckedAt && <span className="text-[12px] text-ink3">Last checked {new Date(v.lastCheckedAt).toLocaleTimeString()}</span>}
            <Button variant="ghost" size="sm" className="ml-auto" onClick={copy}><Clipboard size={13} aria-hidden />Copy diagnostics</Button>
          </div>
        </Disclosure>)}

      <div className="flex flex-wrap items-center justify-end gap-2">
        {(verifying || stuck) && <Button variant="danger" className="mr-auto" onClick={() => setUnpublish(true)}>Unpublish</Button>}
        {verifying && <Button busy={busy === "cancel"} onClick={() => act("cancel")}>Stop checking</Button>}
        {isProvisioning(d.state) && <Button busy={busy === "cancel"} onClick={() => act("cancel")}>Cancel publishing</Button>}
        {(verifying || stuck) && <Button variant={stuck ? "primary" : "secondary"} busy={busy === "retry"} onClick={() => act("retry")}><RefreshCw size={14} aria-hidden />{stuck ? "Check again" : "Check now"}</Button>}
        {failed && onRepublish && <Button variant="primary" onClick={onRepublish}>Try publishing again</Button>}
        {published && <Button onClick={() => api.openExternal(`https://${host}`)}><ExternalLink size={14} aria-hidden />Open website</Button>}
        {onDone && !isProvisioning(d.state) && <Button variant={published ? "primary" : "secondary"} onClick={onDone}>{verifying ? "Keep checking in background" : "Done"}</Button>}
      </div>

      {unpublish && (
        <Confirm title={`Unpublish ${host ?? "this website"}?`} action={`Unpublish ${host ?? "website"}`} onClose={() => setUnpublish(false)}
          onConfirm={async () => { await api.unpublishSite(d.siteId); await refresh(); toast("ok", "Website unpublished. It still runs on this computer."); onDone?.(); }}>
          <p>Hostify removes the DNS record and tunnel route it created for this website. The website keeps running on this computer.</p>
          <p className="mt-2 text-ink2">Records Hostify didn't create are left alone.</p>
        </Confirm>)}
    </div>
  );
}
