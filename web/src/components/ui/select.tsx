import type { ComponentProps } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

type SelectProps = Omit<ComponentProps<"select">, "onChange"> & {
  onValueChange?: (value: string) => void;
};

export const Select = ({
  className,
  children,
  onValueChange,
  ...props
}: SelectProps) => (
  <span className={cn("relative inline-flex h-7", className)}>
    <select
      className="h-full w-full appearance-none rounded-md border border-line bg-surface pl-2 pr-6 text-xs text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 [&>option]:bg-surface [&>option]:text-foreground"
      onChange={(event) => onValueChange?.(event.target.value)}
      {...props}
    >
      {children}
    </select>
    <ChevronDown className="pointer-events-none absolute right-1.5 top-1/2 size-3 -translate-y-1/2 text-muted-foreground" />
  </span>
);
