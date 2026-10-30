import { useEffect, useRef, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, Loader2, X } from "lucide-react";
import { useStore } from "../store";

export function cx(...c: (string | false | undefined | null)[]) { return c.filter(Boolean).join(" "); }

export function fmtBytes(n: number) {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"]; const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${u[i]}`;
}
export function fmtTime(iso: string) {
  const d = new Date(iso); if (isNaN(+d)) return "";
  return d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

type BtnProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "ghost" | "danger"; busy?: boolean; size?: "sm" | "md";
};
export function Button({ variant = "secondary", size = "md", busy, children, className, disabled, ...p }: BtnProps) {
  const v = {
    primary: "bg-kelp text-kelpink hover:brightness-110 border-transparent font-semibold",
    secondary: "bg-panel text-ink border-line hover:bg-panel2",
    ghost: "bg-transparent text-ink2 border-transparent hover:bg-panel2 hover:text-ink",
    danger: "bg-panel text-buoy border-line hover:bg-buoysoft",
  }[variant];
  return (
    <button {...p} disabled={disabled || busy}
      className={cx("inline-flex items-center justify-center gap-2 rounded-ctl border transition-colors disabled:opacity-50 disabled:pointer-events-none",
        size === "sm" ? "h-8 px-3 text-[13px]" : "h-10 px-4", v, className)}>
      {busy && <Loader2 size={15} className="spin" aria-hidden />}
      {children}
    </button>
  );
}

export function IconButton({ label, children, className, ...p }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button {...p} aria-label={label} title={label}
      className={cx("grid h-8 w-8 place-items-center rounded-ctl text-ink2 transition-colors hover:bg-panel2 hover:text-ink disabled:opacity-40", className)}>
      {children}
    </button>
  );
}

export function Card({ children, className }: { children: ReactNode; className?: string }) {
  return <section className={cx("rounded-card border border-line bg-panel", className)}>{children}</section>;
}

export function Field({ label, hint, error, ...p }: InputHTMLAttributes<HTMLInputElement> & { label: string; hint?: string; error?: string }) {
  const id = p.id ?? `f-${label.replace(/\W+/g, "-").toLowerCase()}`;
  return (
    <div className="grid gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-ink">{label}</label>
      <input id={id} {...p} aria-invalid={!!error}
        className={cx("h-10 rounded-ctl border bg-chalk px-3 text-ink placeholder:text-ink3 focus:border-kelp",
          error ? "border-buoy" : "border-line", p.className)} />
      {error ? <p className="text-[13px] text-buoy" role="alert">{error}</p> : hint && <p className="text-[13px] text-ink3">{hint}</p>}
    </div>
  );
}

const STATUS: Record<string, { text: string; cls: string }> = {
  running: { text: "Online", cls: "bg-kelpsoft text-kelp" },
  starting: { text: "Starting", cls: "bg-lanternsoft text-lantern" },
  deploying: { text: "Publishing", cls: "bg-lanternsoft text-lantern" },
  stopped: { text: "Stopped", cls: "bg-panel2 text-ink2" },
  error: { text: "Needs attention", cls: "bg-buoysoft text-buoy" },
};
export function StatusPill({ status }: { status: string }) {
  const s = STATUS[status] ?? STATUS.stopped;
  return (
    <span className={cx("inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-[12px] font-medium", s.cls)}>
      <span className={cx("h-1.5 w-1.5 rounded-full bg-current", (status === "starting" || status === "deploying") && "spin !rounded-none !h-2.5 !w-2.5 border-2 border-current border-t-transparent bg-transparent !rounded-full")} />
      {s.text}
    </span>
  );
}

export function Empty({ icon, title, body, action }: { icon: ReactNode; title: string; body: string; action?: ReactNode }) {
  return (
    <div className="grid place-items-center gap-3 px-6 py-14 text-center">
      <div className="grid h-12 w-12 place-items-center rounded-full bg-panel2 text-ink2">{icon}</div>
      <h3 className="text-lg font-semibold">{title}</h3>
      <p className="max-w-sm text-ink2">{body}</p>
      {action}
    </div>
  );
}

export function Modal({ title, onClose, children, wide, footer }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean; footer?: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input,button,[tabindex]")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "Tab" && ref.current) {
        const f = [...ref.current.querySelectorAll<HTMLElement>("button:not(:disabled),input,a[href],[tabindex='0']")];
        if (!f.length) return;
        const first = f[0], last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("keydown", onKey); prev?.focus(); };
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/45 p-4" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label={title}
        className={cx("rise flex max-h-[92vh] w-full flex-col rounded-card border border-line bg-panel shadow-2xl", wide ? "max-w-3xl" : "max-w-lg")}>
        <header className="flex items-center justify-between border-b border-line px-5 py-3.5">
          <h2 className="text-lg font-semibold">{title}</h2>
          <IconButton label="Close" onClick={onClose}><X size={18} /></IconButton>
        </header>
        <div className="overflow-y-auto p-5">{children}</div>
        {footer && <footer className="flex items-center justify-end gap-2 border-t border-line px-5 py-3.5">{footer}</footer>}
      </div>
    </div>
  );
}

export function Toasts() {
  const { toasts, dismissToast } = useStore();
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-50 grid w-[360px] gap-2" aria-live="polite">
      {toasts.map((t) => {
        const Icon = t.kind === "ok" ? CheckCircle2 : t.kind === "error" ? AlertTriangle : Info;
        const tone = t.kind === "ok" ? "text-kelp" : t.kind === "error" ? "text-buoy" : "text-ink2";
        return (
          <div key={t.id} role={t.kind === "error" ? "alert" : "status"}
            className="rise pointer-events-auto flex items-start gap-3 rounded-card border border-line bg-panel p-3 shadow-lg">
            <Icon size={18} className={cx("mt-0.5 shrink-0", tone)} />
            <p className="flex-1 break-words">{t.text}</p>
            <IconButton label="Dismiss" className="-mr-1 -mt-1 h-7 w-7" onClick={() => dismissToast(t.id)}><X size={15} /></IconButton>
          </div>
        );
      })}
    </div>
  );
}

export function Meter({ value, label, detail }: { value: number; label: string; detail: string }) {
  const v = Math.max(0, Math.min(100, value));
  return (
    <div className="grid gap-2">
      <div className="flex items-baseline justify-between">
        <span className="font-medium">{label}</span>
        <span className="text-[13px] tabular-nums text-ink2">{detail}</span>
      </div>
      <div className="h-2 overflow-hidden rounded-full bg-panel2" role="meter" aria-label={label} aria-valuenow={Math.round(v)} aria-valuemin={0} aria-valuemax={100}>
        <div className={cx("h-full rounded-full transition-[width] duration-700", v > 90 ? "bg-buoy" : v > 75 ? "bg-lantern" : "bg-kelp")} style={{ width: `${v}%` }} />
      </div>
    </div>
  );
}
