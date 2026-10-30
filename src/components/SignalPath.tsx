import { Cloud, Globe, Laptop, Waypoints } from "lucide-react";
import type { ReactNode } from "react";
import { cx } from "./ui";
import type { Site, TunnelHealth } from "../lib/types";

type Hop = "ok" | "idle" | "bad";

/** Derive each hop's state from real data only. Anything we cannot know stays "idle". */
export function hopStates(site: Site, tunnel?: TunnelHealth): { pc: Hop; tunnel: Hop; edge: Hop; domain: Hop } {
  const pc: Hop = site.status === "running" ? "ok" : site.status === "error" ? "bad" : "idle";
  const published = !!site.publicUrl;
  const tun: Hop = !published ? "idle" : tunnel ? (tunnel.connected ? "ok" : "bad") : "idle";
  const edge: Hop = tun === "ok" ? "ok" : tun === "bad" ? "bad" : "idle";
  const domain: Hop = published && pc === "ok" && tun === "ok" ? "ok" : published && tun === "bad" ? "bad" : "idle";
  return { pc, tunnel: tun, edge, domain };
}

function Node({ icon, label, state }: { icon: ReactNode; label: string; state: Hop }) {
  const tone = state === "ok" ? "border-kelp text-kelp" : state === "bad" ? "border-buoy text-buoy" : "border-line text-ink3";
  return (
    <div className="flex w-[84px] shrink-0 flex-col items-center gap-1.5" aria-label={`${label}: ${state === "ok" ? "working" : state === "bad" ? "problem" : "not active"}`}>
      <div className={cx("grid h-10 w-10 place-items-center rounded-full border-2 bg-panel", tone)}>{icon}</div>
      <span className={cx("text-center text-[12px] leading-tight", state === "idle" ? "text-ink3" : "text-ink2")}>{label}</span>
    </div>
  );
}
function Seg({ state }: { state: Hop }) {
  return <div className="mb-5 h-[3px] min-w-6 flex-1 overflow-hidden rounded-full"><div className={cx("h-full w-full", state === "ok" ? "seg-live" : state === "bad" ? "seg-bad" : "seg-idle")} /></div>;
}

export function SignalPath({ site, tunnel, domain }: { site: Site; tunnel?: TunnelHealth; domain?: string }) {
  const h = hopStates(site, tunnel);
  return (
    <div className="flex items-start" role="group" aria-label="Connection path from your PC to the internet">
      <Node icon={<Laptop size={18} />} label="This PC" state={h.pc} />
      <Seg state={h.tunnel} />
      <Node icon={<Waypoints size={18} />} label="Tunnel" state={h.tunnel} />
      <Seg state={h.edge} />
      <Node icon={<Cloud size={18} />} label="Cloudflare" state={h.edge} />
      <Seg state={h.domain} />
      <Node icon={<Globe size={18} />} label={domain ?? "Not published"} state={h.domain} />
    </div>
  );
}
