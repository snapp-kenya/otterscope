import type { HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center rounded-full px-1.5 py-px text-[0.72rem] leading-4",
  {
    variants: {
      variant: {
        neutral: "bg-surface-raised text-muted-foreground",
        ok: "bg-ok-bg text-ok",
        error: "bg-danger-bg text-danger",
        running: "bg-running-bg text-running",
        settling: "bg-warn-bg text-warn",
        outline: "border border-line text-muted-foreground",
      },
    },
    defaultVariants: { variant: "neutral" },
  },
);

type BadgeProps = HTMLAttributes<HTMLSpanElement> &
  VariantProps<typeof badgeVariants>;

export const Badge = ({ className, variant, ...props }: BadgeProps) => (
  <span className={cn(badgeVariants({ variant }), className)} {...props} />
);
