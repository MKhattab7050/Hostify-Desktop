import { useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, CheckCircle2, ChevronDown, Info, Loader2, X } from "lucide-react";
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
export function Button({ variant = "secondary", size = "md", busy, children, className, disabled, type = "button", ...p }: BtnProps) {
  const v = {
    primary: "bg-kelp text-kelpink hover:brightness-110 border-transparent font-semibold",
    secondary: "bg-panel text-ink border-line hover:bg-panel2 hover:border-linestrong",
    ghost: "bg-transparent text-ink2 border-transparent hover:bg-panel2 hover:text-ink",
    danger: "bg-panel text-buoy border-line hover:bg-buoysoft hover:border-buoy",
  }[variant];
  return (
    <button {...p} type={type} disabled={disabled || busy} aria-busy={busy || undefined}
      className={cx("press inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-ctl border disabled:opacity-50 disabled:pointer-events-none",
        size === "sm" ? "h-8 px-3 text-[13px]" : "h-9 px-4", v, className)}>
      {busy && <Loader2 size={15} className="spin" aria-hidden />}
      {children}
    </button>
  );
}

export function IconButton({ label, children, className, type = "button", ...p }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button {...p} type={type} aria-label={label} title={label}
      className={cx("press grid h-8 w-8 place-items-center rounded-ctl text-ink2 hover:bg-panel2 hover:text-ink disabled:opacity-40 disabled:pointer-events-none", className)}>
      {children}
    </button>
  );
}

export function Card({ children, className }: { children: ReactNode; className?: string }) {
  return <section className={cx("rounded-card border border-line bg-panel", className)}>{children}</section>;
}

export function Field({ label, hint, error, ...p }: InputHTMLAttributes<HTMLInputElement> & { label: string; hint?: string; error?: string }) {
  const auto = useId(); const id = p.id ?? auto; const note = `${id}-note`;
  return (
    <div className="grid gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-ink">{label}</label>
      <input id={id} {...p} aria-invalid={!!error} aria-describedby={error || hint ? note : undefined}
        className={cx("h-9 rounded-ctl border bg-chalk px-3 text-ink transition-colors placeholder:text-ink3 focus:border-kelp",
          error ? "border-buoy" : "border-line", p.className)} />
      {error ? <p id={note} className="text-[13px] text-buoy" role="alert">{error}</p> : hint && <p id={note} className="text-[13px] text-ink3">{hint}</p>}
    </div>
  );
}

export function Select({ label, children, className, ...p }: React.SelectHTMLAttributes<HTMLSelectElement> & { label: string }) {
  return (
    <label className={cx("flex items-center gap-2 text-[13px] text-ink2", className)}>{label}
      <select {...p} className="h-8 rounded-ctl border border-line bg-panel px-2 text-ink transition-colors hover:border-linestrong">{children}</select>
    </label>
  );
}

export function Switch({ label, desc, on, onChange, disabled }: { label: string; desc?: string; on: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-6 py-3.5">
      <div><div className="font-medium">{label}</div>{desc && <div className="text-[13px] text-ink2">{desc}</div>}</div>
      <button type="button" role="switch" aria-checked={on} aria-label={label} disabled={disabled} onClick={() => onChange(!on)}
        className={cx("relative h-[22px] w-10 shrink-0 rounded-full transition-colors disabled:opacity-50", on ? "bg-kelp" : "bg-linestrong")}>
        <span className="absolute left-0.5 top-0.5 h-[18px] w-[18px] rounded-full bg-white shadow-sm" style={{ transform: on ? "translateX(18px)" : "none", transition: "transform var(--base) var(--spring)" }} />
      </button>
    </div>
  );
}

const TONE = {
  ok: "bg-kelpsoft text-kelp", wait: "bg-lanternsoft text-lantern", idle: "bg-panel2 text-ink2", bad: "bg-buoysoft text-buoy",
} as const;
export type Tone = keyof typeof TONE;
export function Pill({ tone, busy, children }: { tone: Tone; busy?: boolean; children: ReactNode }) {
  return (
    <span className={cx("inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-medium", TONE[tone])}>
      {busy ? <Loader2 size={11} className="spin" aria-hidden /> : <span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden />}
      {children}
    </span>
  );
}

const STATUS: Record<string, { text: string; tone: Tone; busy?: boolean }> = {
  running: { text: "Online", tone: "ok" },
  starting: { text: "Starting", tone: "wait", busy: true },
  deploying: { text: "Publishing", tone: "wait", busy: true },
  stopped: { text: "Stopped", tone: "idle" },
  error: { text: "Needs attention", tone: "bad" },
};
export function StatusPill({ status }: { status: string }) {
  const s = STATUS[status] ?? STATUS.stopped;
  return <Pill tone={s.tone} busy={s.busy}>{s.text}</Pill>;
}

export function Skeleton({ className }: { className?: string }) { return <div className={cx("skeleton", className)} aria-hidden />; }

export function Disclosure({ title, children, defaultOpen, className }: { title: ReactNode; children: ReactNode; defaultOpen?: boolean; className?: string }) {
  const [open, setOpen] = useState(!!defaultOpen); const id = useId();
  return (
    <div className={className}>
      <button type="button" aria-expanded={open} aria-controls={id} onClick={() => setOpen(!open)}
        className="press flex w-full items-center gap-1.5 rounded-ctl py-1 text-left text-[13px] font-medium text-ink2 hover:text-ink">
        <ChevronDown size={14} aria-hidden style={{ transform: open ? "none" : "rotate(-90deg)", transition: "transform var(--base) var(--ease-out)" }} />{title}
      </button>
      <div id={id} className="disclose" data-open={open}><div inert={!open}>{children}</div></div>
    </div>
  );
}

export function Empty({ icon, title, body, action }: { icon: ReactNode; title: string; body: string; action?: ReactNode }) {
  return (
    <div className="grid place-items-center gap-3 px-6 py-14 text-center">
      <div className="grid h-11 w-11 place-items-center rounded-full bg-panel2 text-ink2">{icon}</div>
      <h3 className="text-lg font-semibold">{title}</h3>
      <p className="max-w-sm text-ink2">{body}</p>
      {action}
    </div>
  );
}

// Open dialogs, innermost last. Only the innermost one reacts to Escape and Tab.
const openDialogs: object[] = [];
const FOCUSABLE = "button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex='0']";

export function Modal({ title, onClose, children, wide, footer }: { title: string; onClose: () => void; children: ReactNode; wide?: boolean; footer?: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  // Keep the latest handler in a ref: callers pass inline closures and re-render on polling,
  // and re-running the focus effect would pull focus out of whatever the user is typing in.
  const close = useRef(onClose); close.current = onClose;
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const self = {}; openDialogs.push(self);
    const body = ref.current?.querySelector<HTMLElement>("[data-modal-body]");
    (body?.querySelector<HTMLElement>(FOCUSABLE) ?? ref.current?.querySelector<HTMLElement>(FOCUSABLE))?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (openDialogs[openDialogs.length - 1] !== self) return;
      if (e.key === "Escape") { e.stopPropagation(); close.current(); }
      if (e.key === "Tab" && ref.current) {
        const f = [...ref.current.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.closest("[inert]"));
        if (!f.length) return;
        const first = f[0], last = f[f.length - 1];
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
      }
    };
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("keydown", onKey); openDialogs.splice(openDialogs.indexOf(self), 1); prev?.focus?.(); };
  }, []);
  // A portal keeps a nested dialog out of its parent's transformed, scrolling box.
  return createPortal(
    <div className="scrim-in fixed inset-0 z-40 grid place-items-center bg-black/45 p-4" onMouseDown={(e) => e.target === e.currentTarget && close.current()}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label={title}
        className={cx("sheet-in flex max-h-[92vh] w-full flex-col rounded-[var(--radius-sheet)] border border-line bg-panel shadow-[var(--shadow-pop)]", wide ? "max-w-[720px]" : "max-w-lg")}>
        <header className="flex items-center justify-between border-b border-line px-5 py-3">
          <h2 className="truncate text-[17px] font-semibold">{title}</h2>
          <IconButton label="Close" onClick={onClose}><X size={17} /></IconButton>
        </header>
        <div data-modal-body className="overflow-y-auto p-5">{children}</div>
        {footer && <footer className="flex items-center justify-end gap-2 border-t border-line px-5 py-3">{footer}</footer>}
      </div>
    </div>,
    document.body,
  );
}

/** Destructive actions name their target and are never the default focus. */
export function Confirm({ title, children, action, onConfirm, onClose }: { title: string; children: ReactNode; action: string; onConfirm: () => Promise<void>; onClose: () => void }) {
  const [busy, setBusy] = useState(false); const [err, setErr] = useState<string>();
  return (
    <Modal title={title} onClose={onClose} footer={<>
      <Button onClick={onClose}>Keep it</Button>
      <Button variant="danger" busy={busy} onClick={async () => { setBusy(true); setErr(undefined); try { await onConfirm(); onClose(); } catch (e) { setErr((e as Error).message); setBusy(false); } }}>{action}</Button>
    </>}>
      {children}
      {err && <p role="alert" className="mt-3 text-buoy">{err}</p>}
    </Modal>
  );
}

export function Toasts() {
  const toasts = useStore((s) => s.toasts); const dismissToast = useStore((s) => s.dismissToast);
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-50 grid w-[360px] max-w-[calc(100vw-32px)] gap-2" aria-live="polite">
      {toasts.map((t) => {
        const Icon = t.kind === "ok" ? CheckCircle2 : t.kind === "error" ? AlertTriangle : Info;
        const tone = t.kind === "ok" ? "text-kelp" : t.kind === "error" ? "text-buoy" : "text-ink2";
        return (
          <div key={t.id} role={t.kind === "error" ? "alert" : "status"}
            className="sheet-in pointer-events-auto flex items-start gap-3 rounded-card border border-line bg-panel p-3 shadow-[var(--shadow-pop)]">
            <Icon size={17} className={cx("mt-0.5 shrink-0", tone)} aria-hidden />
            <p className="flex-1 break-words">{t.text}</p>
            <IconButton label="Dismiss" className="-mr-1 -mt-1 h-7 w-7" onClick={() => dismissToast(t.id)}><X size={14} /></IconButton>
          </div>
        );
      })}
    </div>
  );
}

export function Meter({ value, label, detail }: { value: number; label: string; detail: string }) {
  const v = Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 0;
  return (
    <div className="grid gap-1.5">
      <div className="flex items-baseline justify-between gap-3">
        <span className="font-medium">{label}</span>
        <span className="tnum text-[13px] text-ink2">{detail}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-panel2" role="meter" aria-label={label} aria-valuenow={Math.round(v)} aria-valuemin={0} aria-valuemax={100}>
        <div className={cx("h-full rounded-full transition-[width] duration-700", v > 90 ? "bg-buoy" : v > 75 ? "bg-lantern" : "bg-kelp")} style={{ width: `${v}%` }} />
      </div>
    </div>
  );
}

export function PageHeader({ title, sub, children }: { title: string; sub?: ReactNode; children?: ReactNode }) {
  return (
    <header className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
      <div className="min-w-0"><h1 className="text-[26px] font-bold">{title}</h1>{sub && <p className="mt-1 text-ink2">{sub}</p>}</div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </header>
  );
}
