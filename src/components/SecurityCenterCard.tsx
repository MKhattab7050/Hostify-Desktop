import { useEffect, useState } from "react";
import { AlertTriangle, RefreshCw, ShieldAlert, ShieldCheck } from "lucide-react";
import { api } from "../lib/api";
import type { SecurityPosture } from "../lib/types";
import { Button, Card, Disclosure } from "./ui";

/** This is a read-only audit of locally available evidence, not a penetration test. */
export function SecurityCenterCard() {
  const [report,setReport]=useState<SecurityPosture>();
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string>();
  useEffect(()=>{
    let active=true;
    void api.securityPosture().then((r)=>{if(active)setReport(r)})
      .catch((e)=>{if(active)setError((e as Error).message)});
    return ()=>{active=false};
  },[]);
  async function refresh(){
    setBusy(true);setError(undefined);
    try{setReport(await api.securityPosture());}
    catch(e){setError((e as Error).message);}
    finally{setBusy(false);}
  }
  return <Card className="p-4">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div className="flex items-start gap-2">
        <ShieldAlert size={20} className="mt-0.5 text-lantern" aria-hidden />
        <div>
          <h2 className="text-[16px] font-semibold">Security Center</h2>
          <p className="mt-1 text-[13px] text-ink2">Local evidence and unresolved security risks. This scan never reads secrets or changes your website, DNS or firewall.</p>
        </div>
      </div>
      <Button size="sm" busy={busy} disabled={busy} onClick={()=>void refresh()}>
        <RefreshCw size={13} aria-hidden />Rescan
      </Button>
    </div>
    {report && <>
      <div className="mt-3 grid grid-cols-3 gap-2 text-[13px] sm:max-w-lg">
        <div className="rounded-ctl border border-line p-2"><span className="block text-[20px] font-semibold text-buoy">{report.blockingFindings}</span><span>Blockers</span></div>
        <div className="rounded-ctl border border-line p-2"><span className="block text-[20px] font-semibold text-lantern">{report.warningFindings}</span><span>Warnings</span></div>
        <div className="rounded-ctl border border-line p-2"><span className="block text-[20px] font-semibold">{report.unverifiedChecks}</span><span>Unverified</span></div>
      </div>
      <p className="mt-2 text-[12px] text-ink3">Inspected {report.inspectedSites} local site(s). No blockers does not mean production-ready.</p>
      <Disclosure title={`Evidence and next actions (${report.findings.length})`}>
        <ul className="mt-2 grid gap-2">
          {report.findings.map((finding,i)=><li key={`${finding.siteId||"global"}-${finding.id}-${i}`}
            className="rounded-ctl border border-line p-3">
            <p className="flex items-center gap-2 text-[13px] font-medium">
              {finding.status==="block" ?
                <AlertTriangle size={15} className="shrink-0 text-buoy" aria-hidden /> :
                <ShieldCheck size={15} className="shrink-0 text-lantern" aria-hidden />}
              {finding.title}
            </p>
            <p className="mt-1 text-[12px] text-ink3">{finding.status.toUpperCase()} · {finding.id}{finding.siteId ? ` · site ${finding.siteId.slice(0,8)}` : ""}</p>
            <p className="mt-1 text-[13px] text-ink2">{finding.detail}</p>
            <p className="mt-2 text-[13px]"><strong>Next:</strong> {finding.recommendedAction}</p>
          </li>)}
        </ul>
      </Disclosure>
    </>}
    {!report && !error && <p role="status" className="mt-3 text-[13px] text-ink2">Gathering local security evidence…</p>}
    {error && <p role="alert" className="mt-3 text-[13px] text-buoy">{error}</p>}
  </Card>;
}
