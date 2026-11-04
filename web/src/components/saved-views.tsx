import { useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createView, deleteView, errorMessage } from "@/api";
import { withFilterDefaults, type FilterState } from "@/lib/filters";
import { queryKeys, viewsQuery } from "@/lib/queries";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ErrorNotice } from "@/components/error-notice";

export const SavedViews = ({
  current,
  onApply,
}: {
  current: FilterState;
  onApply: (filters: FilterState) => void;
}) => {
  const queryClient = useQueryClient();
  const viewsResult = useQuery(viewsQuery());
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (viewName: string) => createView(viewName, current),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.views() });
      setOpen(false);
      setName("");
      setFormError(null);
      setActionError(null);
    },
    onError: (error) =>
      setFormError(`Couldn't save view — ${errorMessage(error)}.`),
  });

  const remove = useMutation({
    mutationFn: (id: number) => deleteView(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.views() });
      setActionError(null);
    },
    onError: (error) =>
      setActionError(`Couldn't delete view — ${errorMessage(error)}.`),
  });

  const views = viewsResult.data ?? [];
  const loadError = viewsResult.isError
    ? errorMessage(viewsResult.error)
    : null;
  const hadViews = viewsResult.data !== undefined;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setFormError("A view needs a name.");
      return;
    }
    create.mutate(trimmed);
  };

  return (
    <div className="mt-4 flex flex-wrap items-center gap-2">
      {loadError ? (
        <ErrorNotice onRetry={() => void viewsResult.refetch()}>
          {hadViews ? "Couldn't refresh saved views — " : "Couldn't read saved views — "}
          {loadError}.
        </ErrorNotice>
      ) : null}
      {actionError ? (
        <span className="text-xs text-danger" role="status">
          {actionError}
        </span>
      ) : null}
      {views.map((view) => (
        <span
          key={view.id}
          className="view-chip inline-flex items-center overflow-hidden rounded-full border border-line bg-surface"
        >
          <button
            type="button"
            className="py-1 pl-2.5 pr-1.5 text-[0.78rem] text-primary hover:text-foreground"
            onClick={() => onApply(withFilterDefaults(view.params))}
          >
            {view.name}
          </button>
          <button
            type="button"
            title="delete view"
            aria-label={`delete view ${view.name}`}
            className="py-1 pl-1 pr-2 text-sm text-muted-foreground hover:text-danger"
            onClick={() => remove.mutate(view.id)}
          >
            ×
          </button>
        </span>
      ))}
      <Button
        variant="outline"
        size="sm"
        className="rounded-full border-dashed text-muted-foreground"
        onClick={() => {
          setOpen(true);
          setFormError(null);
        }}
      >
        + save view
      </Button>

      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setName("");
        }}
      >
        <DialogContent>
          <form onSubmit={submit}>
            <DialogHeader>
              <DialogTitle>Save this view</DialogTitle>
              <DialogDescription>
                Stores the current filters as a named preset.
              </DialogDescription>
            </DialogHeader>
            <label
              className="mt-3 block text-xs text-muted-foreground"
              htmlFor="view-name"
            >
              name
            </label>
            <Input
              id="view-name"
              className="mt-1"
              placeholder="e.g. prod errors 24h"
              value={name}
              autoFocus
              onChange={(event) => setName(event.target.value)}
            />
            {formError ? (
              <p className="mt-2 text-xs text-danger">{formError}</p>
            ) : null}
            <DialogFooter>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setOpen(false)}
              >
                cancel
              </Button>
              <Button type="submit" disabled={create.isPending}>
                save view
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
};
