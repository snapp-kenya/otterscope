import type { ReactNode } from "react";

export const PageHeader = ({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children?: ReactNode;
}) => (
  <div className="mt-4 flex flex-wrap items-start gap-3">
    <div className="min-w-0">
      <h2 className="text-base font-medium">{title}</h2>
      {description ? (
        <p className="mt-1 max-w-[48rem] text-xs text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
    {children ? (
      <div className="ml-auto flex items-center gap-2">{children}</div>
    ) : null}
  </div>
);
