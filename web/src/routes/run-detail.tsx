import { useState } from "react";
import { Link, useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { ApiError, errorMessage } from "@/api";
import { runQuery } from "@/lib/queries";
import { Button } from "@/components/ui/button";
import { ErrorNotice, LoadingHint } from "@/components/error-notice";
import { RunView } from "@/components/run-view";
import { SharesPanel } from "@/components/shares-panel";

export const RunDetailPage = () => {
  const { runId } = useParams({ from: "/runs/$runId" });
  const detail = useQuery(runQuery(runId));
  const [shareOpen, setShareOpen] = useState(false);

  const missing =
    detail.error instanceof ApiError && detail.error.status === 404;

  if (missing) {
    return <p className="mt-4 text-sm text-muted-foreground">Run not found.</p>;
  }

  if (detail.isPending) return <LoadingHint />;

  if (detail.isError && detail.data === undefined) {
    return (
      <ErrorNotice onRetry={() => void detail.refetch()}>
        Couldn&apos;t load this run — {errorMessage(detail.error)}.
      </ErrorNotice>
    );
  }

  if (!detail.data) return <LoadingHint />;

  return (
    <div>
      <div className="run-toolbar mt-4 flex items-center gap-4">
        <Link to="/" className="text-xs text-primary hover:underline">
          ← runs
        </Link>
        <Button
          variant="link"
          size="sm"
          onClick={() => setShareOpen((current) => !current)}
        >
          share
        </Button>
      </div>

      {shareOpen ? <SharesPanel key={runId} runId={runId} /> : null}

      {detail.isError ? (
        <ErrorNotice onRetry={() => void detail.refetch()}>
          This run may be out of date — {errorMessage(detail.error)}.
        </ErrorNotice>
      ) : null}

      <RunView detail={detail.data} />
    </div>
  );
};
