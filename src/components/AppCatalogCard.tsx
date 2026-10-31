import { useEffect, useState } from "react";
import { Blocks, Code2, FileCode2, LayoutTemplate, Sparkles } from "lucide-react";
import { api } from "../lib/api";
import type { AppTemplate } from "../lib/types";
import { useStore } from "../store";
import { Button, Card, Pill } from "./ui";

export function AppCatalogCard() {
  const [templates, setTemplates] = useState<AppTemplate[]>([]);
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const [names, setNames] = useState<Record<string,string>>({});
  const refresh = useStore((s)=>s.refresh);
  const go = useStore((s)=>s.go);
  useEffect(()=>{
    let active=true;
    void api.listAppTemplates().then((items)=>{if(active)setTemplates(items);})
      .catch((e)=>{if(active)setError((e as Error).message);});
    return ()=>{active=false;};
  },[]);

  async function add(template: AppTemplate) {
    setError(undefined);setBusy(template.id);
    try {
      const name=(names[template.id] || template.name).trim();
      if (!name) throw new Error("Choose a name for your website.");
      const site=await api.createSiteFromTemplate(template.id,name);
      await refresh();
      go("websites",site.id);
    } catch (e) {setError((e as Error).message);}
    finally {setBusy(undefined);}
  }
  const icons = [LayoutTemplate, Blocks, Code2, FileCode2];
  return (
    <Card className="p-4">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-start gap-2">
          <Sparkles size={18} className="mt-1 text-kelp" aria-hidden />
          <div><h2 className="text-[16px] font-semibold">App Catalog · One-click starters</h2>
            <p className="mt-1 text-[13px] text-ink2">Original offline templates. No external downloads, install scripts, hidden credentials or automatic code trust.</p>
          </div>
        </div>
        <Pill tone="ok">Offline and verifiable</Pill>
      </div>
      {templates.length===0 && !error && <p className="text-[13px] text-ink3">Loading available templates...</p>}
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {templates.map((template,i)=>{
          const Icon=icons[i%icons.length];
          return <div key={template.id} className="flex min-w-0 flex-col gap-2 rounded-ctl border border-line bg-chalk p-3">
            <div className="flex items-center justify-between gap-2">
              <Icon size={22} className="text-kelp" aria-hidden />
              <Pill tone="idle">{template.framework}</Pill>
            </div>
            <div className="text-[15px] font-semibold">{template.name}</div>
            <p className="flex-1 text-[12px] text-ink2">{template.description}</p>
            <label className="grid gap-1 text-[12px] text-ink2">
              Website name
              <input aria-label={`Name for ${template.name}`} className="w-full rounded-ctl border border-line bg-panel px-2 py-1.5 text-ink"
                maxLength={200} value={names[template.id]??template.name}
                onChange={(e)=>setNames((current)=>({...current,[template.id]:e.target.value}))}
                disabled={!!busy} />
            </label>
            <Button size="sm" variant="primary" disabled={!!busy} busy={busy===template.id}
              onClick={()=>void add(template)}>Use template</Button>
            {template.requiresTrust && <p className="text-[11px] text-ink3">Requires explicit trust, runtime and build before starting.</p>}
          </div>;
        })}
      </div>
      {error && <p className="mt-3 text-[13px] text-buoy" role="alert">{error}</p>}
    </Card>
  );
}
