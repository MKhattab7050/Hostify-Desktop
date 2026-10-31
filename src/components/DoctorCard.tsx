import { useState } from "react";
import { Activity, AlertTriangle, HeartPulse } from "lucide-react";
import { api } from "../lib/api";
import type { DoctorReport, Site } from "../lib/types";
import { Button, Card, Disclosure, Pill } from "./ui";

export function DoctorCard({ sites }: { sites: Site[] }) {
  const [results,setResults]=useState<Record<string,DoctorReport>>({});
  const [working,setWorking]=useState<string>();
  const [error,setError]=useState<string>();
  async function analyze(site:Site){
    setWorking(site.id);setError(undefined);
    try{
      const report=await api.doctorAnalyze(site.id);
      setResults((previous)=>({...previous,[site.id]:report}));
    }catch(e){setError((e as Error).message);}
    finally{setWorking(undefined);}
  }
  if(!sites.length)return null;
  return <Card className="p-4">
    <div className="mb-2 flex items-start gap-2">
      <HeartPulse size={20} className="mt-0.5 text-kelp" aria-hidden />
      <div><h2 className="text-[16px] font-semibold">Hostify Doctor</h2>
        <p className="text-[12px] text-ink2">Private, offline log diagnostics with actionable suggestions. No log lines are uploaded, no automated changes or AI model calls.</p>
      </div>
    </div>
    <ul className="divide-y divide-line">
      {sites.map(site=>{
        const report=results[site.id];
        return <li key={site.id} className="py-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2"><span className="font-medium">{site.name}</span>
              {report&&<Pill tone={report.issues.length ? "wait" : "ok"}>
                {report.issues.length ? `${report.issues.length} finding(s)` : "No known signatures"}
              </Pill>}</div>
            <Button size="sm" busy={working===site.id} disabled={!!working} onClick={()=>void analyze(site)}>
              <Activity size={13} />Analyze logs
            </Button>
          </div>
          {report&&<Disclosure title={`Doctor findings · ${report.linesReviewed} lines examined`}>
            {report.issues.length===0?
              <p className="mt-2 text-[13px] text-ink2">No known error signatures found. This is not proof that the website has no problems.</p>:
              <ul className="mt-2 grid gap-2">
                {report.issues.map(issue=><li key={issue.id} className="rounded-ctl border border-line p-3">
                  <p className="flex items-center gap-2 text-[13px] font-semibold"><AlertTriangle size={14} className={issue.severity==="high"?"text-buoy":"text-lantern"} />
                    {issue.headline}</p>
                  <p className="mt-1 text-[12px] text-ink2">{issue.explanation}</p>
                  <p className="mt-2 text-[12px]"><strong>Suggested action:</strong> {issue.action}</p>
                </li>)}
              </ul>}
          </Disclosure>}
        </li>;
      })}
    </ul>
    {error&&<p role="alert" className="mt-2 text-[13px] text-buoy">{error}</p>}
  </Card>;
}
