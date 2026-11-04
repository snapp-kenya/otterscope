import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createAlert,
  deleteAlert,
  errorMessage,
  type Alert,
  type AlertInput,
} from "@/api";
import {
  alertCondition,
  shortWebhook,
} from "@/lib/alerts";
import { alertsQuery, queryKeys } from "@/lib/queries";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { AlertFormDialog } from "@/components/alert-form-dialog";
import { ErrorNotice, LoadingHint } from "@/components/error-notice";
import { PageHeader } from "@/components/page-header";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export const AlertsPage = () => {
  const queryClient = useQueryClient();
  const alertsResult = useQuery(alertsQuery());
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<Alert | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const refreshAlerts = async () => {
    await queryClient.invalidateQueries({ queryKey: queryKeys.alerts() });
  };

  const create = useMutation({
    mutationFn: (body: AlertInput) => createAlert(body),
    onSuccess: async () => {
      await refreshAlerts();
      setDialogOpen(false);
      setEditing(null);
      setFormError(null);
    },
    onError: (error) =>
      setFormError(`Couldn't save alert — ${errorMessage(error)}.`),
  });

  const replace = useMutation({
    mutationFn: async (input: { id: number; body: AlertInput }) => {
      await deleteAlert(input.id);
      return createAlert(input.body);
    },
    onSuccess: async () => {
      await refreshAlerts();
      setDialogOpen(false);
      setEditing(null);
      setFormError(null);
    },
    onError: (error) =>
      setFormError(`Couldn't save alert — ${errorMessage(error)}.`),
  });

  const remove = useMutation({
    mutationFn: (id: number) => deleteAlert(id),
    onSuccess: async () => {
      await refreshAlerts();
      setActionError(null);
    },
    onError: (error) =>
      setActionError(`Couldn't delete alert — ${errorMessage(error)}.`),
  });

  const openCreate = () => {
    setEditing(null);
    setFormError(null);
    setDialogOpen(true);
  };

  const openEdit = (alert: Alert) => {
    setEditing(alert);
    setFormError(null);
    setDialogOpen(true);
  };

  const submit = (body: AlertInput) => {
    setFormError(null);
    if (editing) replace.mutate({ id: editing.id, body });
    else create.mutate(body);
  };

  const alerts = alertsResult.data ?? [];

  return (
    <div>
      <PageHeader
        title="Alerts"
        description="Evaluated over each rule's window; the webhook is notified when it starts and stops firing, after its settle time."
      >
        <Button size="sm" onClick={openCreate}>
          new alert
        </Button>
      </PageHeader>
      <p className="mt-2 text-xs text-muted-foreground">
        Settling stops a value sitting on the threshold from paging twice a
        minute. Slack and Discord webhook URLs are auto-formatted; anything
        else gets a JSON payload.
      </p>

      {actionError ? (
        <p className="mt-2 text-xs text-danger" role="status">
          {actionError}
        </p>
      ) : null}

      {alertsResult.isError ? (
        <ErrorNotice onRetry={() => void alertsResult.refetch()}>
          Couldn&apos;t load alerts — {errorMessage(alertsResult.error)}.
        </ErrorNotice>
      ) : null}

      {alertsResult.isPending ? <LoadingHint /> : null}

      {alertsResult.isSuccess && alerts.length === 0 ? (
        <p className="mt-4 text-sm text-muted-foreground">No alerts yet.</p>
      ) : null}

      {alerts.length > 0 ? (
        <Table className="runs mt-4">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead>state</TableHead>
              <TableHead>name</TableHead>
              <TableHead>condition</TableHead>
              <TableHead>window</TableHead>
              <TableHead>settle</TableHead>
              <TableHead>webhook</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {alerts.map((alert) => (
              <TableRow key={alert.id}>
                <TableCell>
                  <Badge variant={alert.firing ? "error" : "ok"}>
                    {alert.firing ? "firing" : "ok"}
                  </Badge>
                  {alert.pendingSinceNs ? (
                    <Badge
                      variant="settling"
                      className="ml-1.5"
                      title={`Condition changed; waiting ${alert.settleSecs}s for it to hold before notifying`}
                    >
                      settling
                    </Badge>
                  ) : null}
                </TableCell>
                <TableCell>{alert.name}</TableCell>
                <TableCell className="numeric">
                  {alertCondition(alert)}
                </TableCell>
                <TableCell className="numeric">
                  {Math.round(alert.windowSecs / 60)}m
                </TableCell>
                <TableCell className="numeric">
                  {alert.settleSecs
                    ? `${Math.round(alert.settleSecs / 60)}m`
                    : "—"}
                </TableCell>
                <TableCell
                  className="max-w-[16rem] truncate text-muted-foreground"
                  title={alert.webhookUrl}
                >
                  {shortWebhook(alert.webhookUrl)}
                </TableCell>
                <TableCell>
                  <div className="flex items-center justify-end gap-3">
                    <Button
                      variant="link"
                      size="sm"
                      onClick={() => openEdit(alert)}
                    >
                      edit
                    </Button>
                    <Button
                      variant="destructive"
                      size="sm"
                      onClick={() => remove.mutate(alert.id)}
                      disabled={remove.isPending}
                    >
                      delete
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}

      {dialogOpen ? (
        <AlertFormDialog
          onOpenChange={(open) => {
            setDialogOpen(open);
            if (!open) {
              setEditing(null);
              setFormError(null);
            }
          }}
          initial={editing}
          pending={create.isPending || replace.isPending}
          error={formError}
          onSubmit={submit}
        />
      ) : null}
    </div>
  );
};
