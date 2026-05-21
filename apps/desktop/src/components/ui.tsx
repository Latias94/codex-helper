import { Slot } from "@radix-ui/react-slot";
import * as SwitchPrimitives from "@radix-ui/react-switch";
import { type ButtonHTMLAttributes, type HTMLAttributes, type ReactNode } from "react";

import { cn } from "@/lib/utils";

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <section
      className={cn(
        "rounded-2xl border border-slate-200/80 bg-white/92 shadow-[0_18px_55px_rgba(15,23,42,0.055)]",
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("space-y-1.5 p-5 pb-3", className)} {...props} />;
}

export function CardTitle({ className, ...props }: HTMLAttributes<HTMLHeadingElement>) {
  return <h2 className={cn("text-base font-semibold tracking-tight text-slate-950", className)} {...props} />;
}

export function CardDescription({ className, ...props }: HTMLAttributes<HTMLParagraphElement>) {
  return <p className={cn("text-sm leading-6 text-slate-500", className)} {...props} />;
}

export function CardContent({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("p-5 pt-2", className)} {...props} />;
}

type ButtonVariant = "default" | "secondary" | "outline" | "ghost" | "danger" | "warning";

export function Button({
  className,
  variant = "default",
  asChild,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; asChild?: boolean }) {
  const Component = asChild ? Slot : "button";
  return (
    <Component
      className={cn(
        "inline-flex h-9 items-center justify-center gap-2 rounded-xl px-3.5 text-sm font-medium transition",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-500/30 disabled:pointer-events-none disabled:opacity-50",
        variant === "default" && "bg-teal-600 text-white shadow-sm hover:bg-teal-700",
        variant === "secondary" && "bg-slate-100 text-slate-800 hover:bg-slate-200",
        variant === "outline" && "border border-slate-200 bg-white text-slate-700 hover:bg-slate-50",
        variant === "ghost" && "text-slate-600 hover:bg-slate-100",
        variant === "danger" && "bg-red-600 text-white shadow-sm hover:bg-red-700",
        variant === "warning" && "border border-amber-200 bg-amber-50 text-amber-700 hover:bg-amber-100",
        className,
      )}
      {...props}
    />
  );
}

type BadgeVariant = "default" | "success" | "warning" | "danger" | "muted" | "teal" | "blue";

export function Badge({
  className,
  variant = "default",
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full border px-2.5 py-1 text-xs font-medium",
        variant === "default" && "border-slate-200 bg-slate-50 text-slate-600",
        variant === "success" && "border-emerald-200 bg-emerald-50 text-emerald-700",
        variant === "warning" && "border-amber-200 bg-amber-50 text-amber-700",
        variant === "danger" && "border-red-200 bg-red-50 text-red-700",
        variant === "muted" && "border-slate-200 bg-slate-100 text-slate-500",
        variant === "teal" && "border-teal-200 bg-teal-50 text-teal-700",
        variant === "blue" && "border-sky-200 bg-sky-50 text-sky-700",
        className,
      )}
      {...props}
    />
  );
}

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-9 rounded-xl border border-slate-200 bg-white px-3 text-sm text-slate-800 shadow-sm outline-none",
        "placeholder:text-slate-400 focus:border-teal-300 focus:ring-2 focus:ring-teal-500/15",
        className,
      )}
      {...props}
    />
  );
}

export function SelectBox({ className, children, ...props }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={cn(
        "h-9 rounded-xl border border-slate-200 bg-white px-3 text-sm text-slate-800 shadow-sm outline-none",
        "focus:border-teal-300 focus:ring-2 focus:ring-teal-500/15",
        className,
      )}
      {...props}
    >
      {children}
    </select>
  );
}

export function Switch({ checked }: { checked: boolean }) {
  return (
    <SwitchPrimitives.Root
      checked={checked}
      className={cn(
        "relative h-6 w-11 rounded-full border transition-colors",
        checked ? "border-teal-500 bg-teal-600" : "border-slate-200 bg-slate-200",
      )}
    >
      <SwitchPrimitives.Thumb
        className={cn(
          "block h-5 w-5 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-5" : "translate-x-0.5",
        )}
      />
    </SwitchPrimitives.Root>
  );
}

export function Separator({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("h-px bg-slate-200", className)} {...props} />;
}

export function Segment({
  items,
  value,
  className,
}: {
  items: Array<string>;
  value: string;
  className?: string;
}) {
  return (
    <div className={cn("inline-flex overflow-hidden rounded-xl border border-slate-200 bg-slate-50 p-0.5", className)}>
      {items.map((item) => (
        <span
          key={item}
          className={cn(
            "min-w-20 px-3 py-1.5 text-center text-sm text-slate-500",
            item === value && "rounded-lg bg-white text-teal-700 shadow-sm ring-1 ring-teal-200",
          )}
        >
          {item}
        </span>
      ))}
    </div>
  );
}

export function TooltipHint({ children, content }: { children: ReactNode; content: ReactNode }) {
  return (
    <span className="group relative inline-flex">
      {children}
      <span className="pointer-events-none absolute bottom-full left-1/2 z-20 mb-2 hidden w-64 -translate-x-1/2 rounded-xl bg-slate-950 p-3 text-xs leading-5 text-white shadow-xl group-hover:block">
        {content}
      </span>
    </span>
  );
}
