import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

export const Table = ({ className, ...props }: ComponentProps<"table">) => (
  <div className="w-full overflow-x-auto">
    <table
      className={cn("w-full border-collapse text-xs", className)}
      {...props}
    />
  </div>
);

export const TableHeader = ({
  className,
  ...props
}: ComponentProps<"thead">) => <thead className={className} {...props} />;

export const TableBody = ({
  className,
  ...props
}: ComponentProps<"tbody">) => <tbody className={className} {...props} />;

export const TableRow = ({ className, ...props }: ComponentProps<"tr">) => (
  <tr
    className={cn(
      "border-b border-line-faint last:border-b-0",
      className,
    )}
    {...props}
  />
);

export const TableHead = ({ className, ...props }: ComponentProps<"th">) => (
  <th
    className={cn(
      "border-b border-line px-2.5 py-1.5 text-left font-medium text-muted-foreground",
      className,
    )}
    {...props}
  />
);

export const TableCell = ({ className, ...props }: ComponentProps<"td">) => (
  <td className={cn("px-2.5 py-1.5 align-middle", className)} {...props} />
);
