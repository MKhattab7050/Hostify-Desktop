import { useEffect, useState } from "react";
import { GitBranch, RefreshCw, ShieldCheck, XCircle } from "lucide-react";
import { api } from "../lib/api";
import type { GithubPendingPush, GithubSource, Site } from "../lib/types";
import { Button, Card, Disclosure, Field } from "./ui";

const defaultSource: GithubSource = {repository:"",branch:"main",enabled:false};
export function GithubSourcesCard({ sites }: { sites: Site[] }) {
  const [sources,setSources]=useState<Record<string,GithubSource>>({});
  const [pending,setPending]=useState<Record<string,GithubPendingPush[]>>({});
  const [busy,setBusy]=useState<string>();
  const [error,setError]=useState<string>();
  const [notice,setNotice]=useState<string>();
  useEffect(()=>{
    let active=true;
    void Promise.all(sites.map(async (site)=>({
      id:site.id,
      source:await api.githubSourceStatus(site.id),
      pushes:await api.githubPendingPushes(site.id),
    }))).then((results)=>{
      if(!active)return;
      setSources(Object.fromEntries(results.map(x=>[x.id,x.source])));
      setPending(Object.fromEntries(results.map(x=>[x.id,x.pushes])));
    }).catch((e)=>{if(active)setError((e as Error).message);});
    return ()=>{active=false;};
  },[sites.map(s=>s.id).join("|")]);
  const update=(id:string,patch:Partial<GithubSource>)=>
    setSources(prev=>({...prev,[id]:{...(prev[id]??defaultSource),...patch}}));
  async function save(site:Site,enabled:boolean){
    setBusy(site.id);setError(undefined);setNotice(undefined);
    try{
      const draft=sources[site.id]??defaultSource;
      if(enabled && (!/^[a-z0-9_.-]+\/[a-z0-9_.-]+$/i.test(draft.repository) ||
          !/^[a-z0-9_./-]+$/i.test(draft.branch))) {
        throw new Error("Enter a valid owner/repository and branch before enabling.");
      }
      const next=await api.githubConfigureSource(site.id,draft.repository,draft.branch,enabled);
      update(site.id,next);
      setPending(prev=>({...prev,[site.id]:(prev[site.id]??[]).map(p=>({
        ...p,state:(!enabled||p.repository.toLowerCase()!==next.repository.toLowerCase()||p.branch!==next.branch)&&p.state==="awaiting_review"?"rejected":p.state
      }))}));
      setNotice(enabled?`GitHub push review enabled for ${site.name}. Automatic builds remain disabled.`:
        `GitHub push review disabled for ${site.name}.`);
    }catch(e){setError((e as Error).message);}
    finally{setBusy(undefined);}
  }
  async function reload(site:Site){
    setBusy(site.id);setError(undefined);
    try{
      const result=await api.githubPendingPushes(site.id);
      setPending(prev=>({...prev,[site.id]:result}));
    }catch(e){setError((e as Error).message);}
    finally{setBusy(undefined);}
  }
  async function reject(site:Site,push:GithubPendingPush){
    setBusy(site.id);setError(undefined);
    try{
      const result=await api.githubRejectPush(site.id,push.deliveryId);
      if(!result.rejected)throw new Error("This push is no longer awaiting review.");
      await reload(site);
      setNotice(`Push ${push.commitSha.slice(0,10)} rejected. No code was executed.`);
    }catch(e){setError((e as Error).message);}
    finally{setBusy(undefined);}
  }
  return <Card className="p-4">
    <div className="mb-3 flex items-start gap-2">
      <GitBranch size={19} aria-hidden className="mt-0.5 text-kelp"/>
      <div><h2 className="text-[16px] font-semibold">GitHub · Source review</h2>
        <p className="mt-1 text-[13px] text-ink2">Per-website opt-in. A signed Push can enter a review inbox, but this desktop version does not expose a webhook listener, download code or deploy automatically.</p>
      </div>
    </div>
    <ul className="divide-y divide-line">
      {sites.map(site=>{
        const source=sources[site.id]??defaultSource;
        const pushes=pending[site.id]??[];
        return <li key={site.id} className="py-3">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <span className="font-medium">{site.name}</span>
            <span className="flex items-center gap-1 text-[12px] text-ink2">
              <ShieldCheck size={13} aria-hidden/>
              {source.enabled?"Review enabled":"Not connected"}
            </span>
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            <Field label={`Repository for ${site.name}`} value={source.repository}
              placeholder="owner/repository" disabled={!!busy} onChange={e=>update(site.id,{repository:e.target.value})}/>
            <Field label={`Branch for ${site.name}`} value={source.branch}
              disabled={!!busy} onChange={e=>update(site.id,{branch:e.target.value})}/>
          </div>
          <div className="mt-2 flex flex-wrap gap-2">
            <Button size="sm" busy={busy===site.id} disabled={!!busy}
              onClick={()=>void save(site,source.enabled)}>Save source</Button>
            {source.enabled?
              <Button size="sm" variant="danger" disabled={!!busy}
                onClick={()=>void save(site,false)}>Disable and reject pending</Button>:
              <Button size="sm" variant="primary" disabled={!!busy}
                onClick={()=>void save(site,true)}>Enable push review</Button>}
          </div>
          <Disclosure title={`Push inbox (${pushes.filter(x=>x.state==="awaiting_review").length} awaiting review)`}>
            <div className="mt-2 flex items-center justify-between gap-2">
              <p className="text-[12px] text-ink2">Verified Push requests only. Approval and deployment are not implemented.</p>
              <Button size="sm" disabled={!!busy} onClick={()=>void reload(site)}>
                <RefreshCw size={13} aria-hidden/>Refresh
              </Button>
            </div>
            {!pushes.length ? <p className="mt-2 text-[12px] text-ink3">No GitHub pushes recorded.</p>:
              <ul className="mt-2 grid gap-2">
                {pushes.map(push=><li key={push.deliveryId} className="flex flex-wrap items-center justify-between gap-2 rounded-ctl border border-line p-2">
                  <div>
                    <p className="text-[13px] font-medium">{push.commitSha.slice(0,12)}</p>
                    <p className="text-[12px] text-ink2">{push.repository} · {push.branch} · {push.state.replace(/_/g," ")}</p>
                  </div>
                  {push.state==="awaiting_review"&&<Button size="sm" variant="danger" disabled={!!busy}
                    onClick={()=>void reject(site,push)}><XCircle size={13} aria-hidden/>Reject push</Button>}
                </li>)}
              </ul>}
          </Disclosure>
        </li>;
      })}
    </ul>
    {error&&<p role="alert" className="mt-2 text-[13px] text-buoy">{error}</p>}
    {notice&&<p role="status" className="mt-2 text-[13px] text-kelp">{notice}</p>}
  </Card>;
}
