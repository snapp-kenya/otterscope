import { useQuery } from "@tanstack/react-query";
import { errorMessage } from "@/api";
import { fmtStart } from "@/lib/format";
import { auditQuery } from "@/lib/queries";
import { Badge } from "@/components/ui/badge";
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

export const AuditPage = () => {
  const auditResult = useQuery(auditQuery());
  const entries = auditResult.data ?? [];

  return (
    <div>
      <PageHeader
        title="Audit log"
        description="Every create/delete of assertions, alerts, projects, tokens, views, and shares — from the UI, API, or CLI."
      />

      {auditResult.isError ? (
        <ErrorNotice onRetry={() => void auditResult.refetch()}>
          Couldn&apos;t {auditResult.data === undefined ? "load" : "refresh"}{" "}
          the audit log — {errorMessage(auditResult.error)}.
        </ErrorNotice>
      ) : null}

      {auditResult.isPending ? <LoadingHint /> : null}

      {auditResult.isSuccess && entries.length === 0 ? (
        <p className="mt-4 text-sm text-muted-foreground">
          No changes recorded yet.
        </p>
      ) : null}

      {entries.length > 0 ? (
        <Table className="runs mt-4">
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead>when</TableHead>
              <TableHead>action</TableHead>
              <TableHead>entity</TableHead>
              <TableHead>detail</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {entries.map((entry, index) => (
              <TableRow
                key={`${entry.at}-${entry.action}-${entry.entity}-${index}`}
              >
                <TableCell className="numeric text-muted-foreground">
                  {fmtStart(entry.at)}
                </TableCell>
                <TableCell>
                  <Badge
                    variant={entry.action === "delete" ? "error" : "ok"}
                  >
                    {entry.action}
                  </Badge>
                </TableCell>
                <TableCell>{entry.entity}</TableCell>
                <TableCell
                  className="max-w-[24rem] truncate text-muted-foreground"
                  title={entry.detail}
                >
                  {entry.detail}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
    </div>
  );
};
