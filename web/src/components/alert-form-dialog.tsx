import { useState, type FormEvent } from "react";
import type { Alert, AlertInput } from "@/api";
import {
  alertSettles,
  alertTypeHint,
  alertTypes,
  alertWindows,
  draftFromAlert,
  emptyAlertDraft,
  validateAlertDraft,
  type AlertDraft,
} from "@/lib/alerts";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export const AlertFormDialog = ({
  onOpenChange,
  initial,
  pending,
  error,
  onSubmit,
}: {
  onOpenChange: (open: boolean) => void;
  initial: Alert | null;
  pending: boolean;
  error: string | null;
  onSubmit: (body: AlertInput) => void;
}) => {
  const [draft, setDraft] = useState<AlertDraft>(() =>
    initial ? draftFromAlert(initial) : emptyAlertDraft(),
  );
  const [formError, setFormError] = useState<string | null>(null);

  const update = (patch: Partial<AlertDraft>) =>
    setDraft((current) => ({ ...current, ...patch }));

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const parsed = validateAlertDraft(draft);
    if (!parsed.ok) {
      setFormError(parsed.error);
      return;
    }
    setFormError(null);
    onSubmit(parsed.body);
  };

  const hint = alertTypeHint(draft.type);

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(40rem,calc(100vw-2rem))]">
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>{initial ? "Edit alert" : "New alert"}</DialogTitle>
            <DialogDescription>
              {initial
                ? "Saving removes the existing rule and creates the new one. The stored webhook URL is redacted, so re-enter it."
                : "Evaluated over its window; notifies the webhook when it starts and stops firing."}
            </DialogDescription>
          </DialogHeader>

          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Input
              placeholder="name"
              aria-label="alert name"
              className="w-[12rem]"
              value={draft.name}
              onChange={(event) => update({ name: event.target.value })}
            />
            <Select
              className="w-[13rem]"
              aria-label="alert type"
              value={draft.type}
              onValueChange={(value) => update({ type: value })}
            >
              {alertTypes.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </Select>
            <Input
              placeholder="threshold"
              aria-label="threshold"
              title={hint}
              className="w-[6rem]"
              value={draft.threshold}
              onChange={(event) => update({ threshold: event.target.value })}
            />
            {draft.type === "assertion_fail_rate" ? (
              <Input
                placeholder="assertion name"
                aria-label="assertion name"
                className="w-[11rem]"
                value={draft.config}
                onChange={(event) => update({ config: event.target.value })}
              />
            ) : null}
            <Select
              className="w-[8rem]"
              aria-label="alert window"
              value={String(draft.windowSecs)}
              onValueChange={(value) =>
                update({ windowSecs: Number(value) })
              }
            >
              {alertWindows.map((option) => (
                <option key={option.value} value={String(option.value)}>
                  over {option.label}
                </option>
              ))}
            </Select>
            <Select
              className="w-[11rem]"
              aria-label="settle time"
              title="How long the condition must hold before notifying, in both directions"
              value={String(draft.settleSecs)}
              onValueChange={(value) =>
                update({ settleSecs: Number(value) })
              }
            >
              {alertSettles.map((option) => (
                <option key={option.value} value={String(option.value)}>
                  {option.label}
                </option>
              ))}
            </Select>
            <Input
              placeholder={
                initial
                  ? "webhook URL (re-enter; stored value is redacted)"
                  : "webhook URL (Slack/Discord/…)"
              }
              aria-label="webhook URL"
              className="min-w-[16rem] flex-1"
              value={draft.webhookUrl}
              onChange={(event) => update({ webhookUrl: event.target.value })}
            />
          </div>

          <p className="mt-2 text-xs text-muted-foreground">{hint}</p>
          {formError ? (
            <p className="mt-2 text-xs text-danger" role="status">
              {formError}
            </p>
          ) : null}
          {error ? (
            <p className="mt-2 text-xs text-danger" role="status">
              {error}
            </p>
          ) : null}

          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              onClick={() => onOpenChange(false)}
            >
              cancel
            </Button>
            <Button
              type="submit"
              disabled={pending || !draft.name || !draft.webhookUrl}
            >
              {initial ? "save changes" : "add alert"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
};
