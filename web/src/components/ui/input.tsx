import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export const Input = ({ className, type, ...props }: ComponentProps<"input">) => (
  <input
    type={type}
    className={cn(
      "h-7 w-full min-w-0 rounded-md border border-line bg-surface px-2 text-xs text-foreground placeholder:text-muted-foreground/70 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
      className,
    )}
    {...props}
  />
);
