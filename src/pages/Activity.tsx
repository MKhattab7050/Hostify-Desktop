import { useEffect, useState } from "react";
import { ScrollText } from "lucide-react";
import { useStore } from "../store";
import { api } from "../lib/api";
import { Card, Empty, cx, fmtTime } from "../components/ui";

export function Activity() {
  const { activity, sites } = useStore();
  const [filter, setFilter] = useState<string>("all");
  const [logs, setLogs] = useState<string[]>([]);
  useEffect(() => {
    if (filter === "all") { setLogs([]); return; }
    let live = true; const load = () => api.siteLogs(filter, 300).then((l) => live && setLogs(l)).catch(() => {});
    void load(); const t = setInterval(load, 3000); return () => { live = false; clearInterval(t); };
  }, [filter]);
  const rows = filter === "all" ? activity : activity.filter((a) => a.siteId === filter);
  return (
    <div className="mx-auto grid max-w-[1100px] gap-5">
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-bold">Activity & Logs</h1>
        <label className="flex items-center gap-2 text-ink2">Show
          <select value={filter} onChange={(e) => setFilter(e.target.value)} className="h-9 rounded-ctl border border-line bg-panel px-2 text-ink">
            <option value="all">Everything</option>{sites.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
          </select></label>
      </div>
      <Card>
        {rows.length === 0 ? <Empty icon={<ScrollText size={22} />} title="No activity yet" body="Starting, stopping, publishing and errors are recorded here." /> : (
          <ul>{rows.map((a) => (
            <li key={a.id} className="flex gap-4 border-b border-line px-5 py-3 last:border-0">
              <span className={cx("mt-2 h-2 w-2 shrink-0 rounded-full", a.level === "error" ? "bg-buoy" : a.level === "warn" ? "bg-lantern" : "bg-kelp")} aria-label={a.level} />
              <p className="selectable flex-1 break-words">{a.message}</p><time className="shrink-0 text-[13px] text-ink3">{fmtTime(a.ts)}</time>
            </li>))}</ul>
        )}
      </Card>
      {filter !== "all" && (
        <Card className="p-0"><h2 className="border-b border-line px-5 py-3 font-semibold">Server log</h2>
          <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all p-5 font-mono text-[12px] leading-5 text-ink2">{logs.join("\n") || "No log lines yet."}</pre></Card>
      )}
    </div>
  );
}
