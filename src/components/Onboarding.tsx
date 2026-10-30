import { useEffect, useState } from "react";
import { Check, Loader2, X } from "lucide-react";
import { useStore } from "../store";
import { listen } from "../lib/api";
import { Button, cx, fmtBytes } from "./ui";
import { Logo } from "./Sidebar";

type Row = { label: string; detail: string; state: "ok" | "bad" | "wait" };

function CheckRow({ label, detail, state }: Row) {
  return (
    <li className="flex items-start gap-3 py-3">
      <span className={cx("grid h-6 w-6 shrink-0 place-items-center rounded-full", state === "ok" ? "bg-kelpsoft text-kelp" : state === "bad" ? "bg-buoysoft text-buoy" : "bg-panel2 text-ink2")}>
        {state === "ok" ? <Check size={13} strokeWidth={3} aria-label="Ready" /> : state === "bad" ? <X size={13} strokeWidth={3} aria-label="Needs attention" /> : <Loader2 size={13} className="spin" aria-label="Checking" />}</span>
      <div className="min-w-0"><div className="font-medium">{label}</div><div className="break-words text-[13px] text-ink2">{detail}</div></div>
    </li>
  );
}

/** First run only. Hands over to the wizard, which walks through add, preview, domain, publish and verify. */
export function Onboarding({ onFinish }: { onFinish: () => void }) {
  const engine = useStore((s) => s.engine); const engineError = useStore((s) => s.engineError); const stats = useStore((s) => s.stats);
  const install = useStore((s) => s.installComponents); const openWizard = useStore((s) => s.openWizard);
  const [step, setStep] = useState(0); const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  const [progress, setProgress] = useState<{ name: string; percent: number }>();
  useEffect(() => {
    let off: (() => void) | undefined; let dead = false;
    listen<{ name: string; percent: number }>("components-progress", setProgress).then((u) => (dead ? u() : (off = u)));
    return () => { dead = true; off?.(); };
  }, []);

  const free = stats ? stats.diskTotalBytes - stats.diskUsedBytes : undefined;
  const rows: Row[] = [
    { label: "Hosting engine", detail: engineError ?? (engine?.running ? "Running in the background on this PC." : "Starting"), state: engineError ? "bad" : engine?.running ? "ok" : "wait" },
    { label: "Disk space", detail: free === undefined ? "Checking" : `${fmtBytes(free)} free. Hostify needs about 150 MB plus your websites.`, state: free === undefined ? "wait" : free < 200e6 ? "bad" : "ok" },
    { label: "Web server (Caddy)", detail: engine?.caddy?.ready ? `Ready, version ${engine.caddy.version ?? "unknown"}.` : engine?.caddy?.error ?? "Not installed yet. It serves your websites on this PC.", state: engine?.caddy?.ready ? "ok" : "bad" },
    { label: "Tunnel connector (cloudflared)", detail: engine?.cloudflared?.ready ? `Ready, version ${engine.cloudflared.version ?? "unknown"}.` : engine?.cloudflared?.error ?? "Not installed yet. It connects your websites to your domain.", state: engine?.cloudflared?.ready ? "ok" : "bad" },
  ];
  const ready = !!engine?.componentsReady;

  async function run() {
    setBusy(true); setErr(undefined);
    try { await install(); } catch (e) { setErr((e as Error).message); } finally { setBusy(false); setProgress(undefined); }
  }
  const start = () => { onFinish(); openWizard(); };

  return (
    <div className="scrim-in fixed inset-0 z-30 grid place-items-center overflow-y-auto bg-chalk p-6">
      <div className="sheet-in w-full max-w-[560px]" key={step}>
        <div className="mb-6 flex items-center gap-2.5"><Logo /><span className="font-display text-xl font-bold tracking-tight">Hostify</span></div>
        {step === 0 ? (<>
          <h1 className="text-[34px] font-bold leading-[1.1]">Host a website from this PC, on your own domain.</h1>
          <p className="mt-3 text-[15px] text-ink2">Your website stays on your computer. A secure tunnel to Cloudflare puts it online over HTTPS, without opening any ports on your router.</p>
          <p className="mt-3 text-ink2">You'll add your files, preview them, connect your domain, and publish. Only the last two need a Cloudflare account.</p>
          <div className="mt-7 flex items-center gap-2"><Button variant="primary" onClick={() => setStep(1)}>Check this computer</Button><Button variant="ghost" onClick={onFinish}>Skip setup</Button></div>
        </>) : (<>
          <h1 className="text-[26px] font-bold">Check this computer</h1>
          <ul className="mt-3 divide-y divide-line rounded-card border border-line bg-panel px-4" aria-live="polite">{rows.map((r) => <CheckRow key={r.label} {...r} />)}</ul>
          {!ready && <p className="mt-3 text-[13px] text-ink2">Hostify downloads Caddy and cloudflared from their official releases and checks each file against a pinned checksum before using it.</p>}
          {err && <p role="alert" className="selectable mt-3 text-buoy">{err}</p>}
          <div className="mt-6 flex items-center gap-2">
            {ready ? <Button variant="primary" onClick={start}>Add your first website</Button>
              : <Button variant="primary" busy={busy} onClick={run}>{busy ? (progress ? `Installing ${progress.name}: ${progress.percent}%` : "Installing") : err ? "Try installing again" : "Install components"}</Button>}
            <Button variant="ghost" onClick={() => setStep(0)}>Back</Button>
            {!ready && <Button variant="ghost" className="ml-auto" onClick={onFinish}>Skip for now</Button>}
          </div>
        </>)}
      </div>
    </div>
  );
}
