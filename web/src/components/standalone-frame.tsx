import type { ReactNode } from "react";

export const StandaloneFrame = ({
  children,
  tagline,
}: {
  children: ReactNode;
  tagline: string;
}) => (
  <div className="mx-auto max-w-[60rem] px-6 py-8">
    <header className="flex items-baseline gap-4 border-b border-line pb-3">
      <h1 className="m-0 text-[1.4rem] leading-tight">🦦 Otterscope</h1>
      <span className="text-xs text-muted-foreground">{tagline}</span>
    </header>
    <main className="pt-3">{children}</main>
  </div>
);
