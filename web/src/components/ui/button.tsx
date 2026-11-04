import type { ButtonHTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1 whitespace-nowrap rounded-md text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-40 [&_svg]:size-3.5 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default:
          "bg-primary font-semibold text-primary-foreground hover:bg-primary/90",
        outline:
          "border border-line bg-transparent text-muted-foreground hover:border-primary hover:text-foreground",
        ghost:
          "text-muted-foreground hover:bg-surface-raised hover:text-foreground",
        destructive: "text-danger hover:text-danger/80",
        link: "h-auto p-0 text-primary hover:underline",
      },
      size: {
        default: "h-7 px-2.5",
        sm: "h-6 px-2 text-[0.72rem]",
        icon: "size-7",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof buttonVariants>;

export const Button = ({
  className,
  variant,
  size,
  ...props
}: ButtonProps) => (
  <button
    className={cn(buttonVariants({ variant, size }), className)}
    {...props}
  />
);
