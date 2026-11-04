import {
  createContext,
  useContext,
  useEffect,
  useRef,
  type ComponentProps,
  type ReactNode,
} from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/utils";

type DialogContextValue = { onOpenChange: (open: boolean) => void };

const DialogContext = createContext<DialogContextValue | null>(null);

export const Dialog = ({
  open,
  onOpenChange,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}) => {
  const dialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const cancel = (event: Event) => {
      event.preventDefault();
      onOpenChange(false);
    };
    dialog.addEventListener("cancel", cancel);
    return () => dialog.removeEventListener("cancel", cancel);
  }, [onOpenChange]);

  return (
    <dialog
      ref={dialogRef}
      className="m-auto max-h-[calc(100vh-2rem)] w-fit max-w-[calc(100vw-2rem)] overflow-y-auto rounded-lg border border-line bg-popover p-0 text-popover-foreground shadow-2xl backdrop:bg-black/60"
      onClick={(event) => {
        if (event.target === dialogRef.current) onOpenChange(false);
      }}
    >
      <DialogContext.Provider value={{ onOpenChange }}>
        <div className="p-4">{children}</div>
      </DialogContext.Provider>
    </dialog>
  );
};

export const DialogContent = ({
  className,
  children,
  ...props
}: ComponentProps<"div">) => {
  const context = useContext(DialogContext);

  return (
    <div
      className={cn(
        "relative w-[min(34rem,calc(100vw-2rem))]",
        className,
      )}
      {...props}
    >
      {children}
      <button
        type="button"
        aria-label="close"
        className="absolute right-0 top-0 rounded p-1 text-muted-foreground hover:text-foreground"
        onClick={() => context?.onOpenChange(false)}
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
};

export const DialogHeader = ({
  className,
  ...props
}: ComponentProps<"div">) => (
  <div className={cn("flex flex-col gap-1", className)} {...props} />
);

export const DialogFooter = ({
  className,
  ...props
}: ComponentProps<"div">) => (
  <div
    className={cn("mt-4 flex items-center justify-end gap-2", className)}
    {...props}
  />
);

export const DialogTitle = ({
  className,
  ...props
}: ComponentProps<"h2">) => (
  <h2 className={cn("text-sm font-medium", className)} {...props} />
);

export const DialogDescription = ({
  className,
  ...props
}: ComponentProps<"p">) => (
  <p className={cn("text-xs text-muted-foreground", className)} {...props} />
);
