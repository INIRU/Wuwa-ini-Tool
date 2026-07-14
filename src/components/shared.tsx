import { AlertTriangle, CheckCircle2, Info, LoaderCircle } from "lucide-react";
import type { ReactNode } from "react";

export function PageHeader({ title, description, actions }: { title: string; description: string; actions?: ReactNode }) {
  return <header className="page-header"><div><h1>{title}</h1><p>{description}</p></div>{actions ? <div className="page-header__actions">{actions}</div> : null}</header>;
}

export function Notice({ kind = "info", children }: { kind?: "info" | "warning" | "success"; children: ReactNode }) {
  const Icon = kind === "warning" ? AlertTriangle : kind === "success" ? CheckCircle2 : Info;
  return <div className={`notice notice--${kind}`} role={kind === "warning" ? "alert" : "status"}><Icon aria-hidden="true" size={18} /><div>{children}</div></div>;
}

export function LoadingState({ label = "Loading…" }: { label?: string }) {
  return <div className="state-panel" role="status"><LoaderCircle aria-hidden="true" className="spin" size={20} />{label}</div>;
}

export function EmptyState({ title, description }: { title: string; description: string }) {
  return <div className="state-panel"><strong>{title}</strong><span>{description}</span></div>;
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return <label className="field"><span>{label}</span>{children}{hint ? <small>{hint}</small> : null}</label>;
}
