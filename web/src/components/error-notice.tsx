import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";

export const ErrorNotice = ({
  children,
  onRetry,
}: {
  children: ReactNode;
  onRetry?: () => void;
}) => (
  <div className="my-2 flex flex-wrap items-baseline gap-3" role="status">
    <span className="rounded-md bg-danger-bg px-2 py-1 text-xs text-danger">
      {children}
    </span>
    {onRetry ? (
      <Button variant="link" size="sm" onClick={onRetry}>
        retry
      </Button>
    ) : null}
  </div>
);

export const LoadingHint = ({
  children = "loading…",
}: {
  children?: ReactNode;
}) => <p className="mt-4 text-sm text-muted-foreground">{children}</p>;
