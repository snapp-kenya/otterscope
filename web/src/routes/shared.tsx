import { useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { ApiError, errorMessage } from "@/api";
import { sharedRunQuery } from "@/lib/queries";
import { ErrorNotice, LoadingHint } from "@/components/error-notice";
import { RunView } from "@/components/run-view";
import { StandaloneFrame } from "@/components/standalone-frame";

const SharedRunView = ({ token }: { token: string }) => {
  const detail = useQuery(sharedRunQuery(token));
  const missing =
    detail.error instanceof ApiError && detail.error.status === 404;

  return (
    <StandaloneFrame tagline="shared run · read-only">
      {missing ? (
        <p className="mt-4 text-sm text-muted-foreground">
          This shared run doesn’t exist or was revoked.
        </p>
      ) : detail.isError ? (
        <ErrorNotice onRetry={() => void detail.refetch()}>
          Couldn&apos;t load this shared run — {errorMessage(detail.error)}.
        </ErrorNotice>
      ) : detail.isPending ? (
        <LoadingHint />
      ) : detail.data ? (
        <RunView detail={detail.data} />
      ) : (
        <LoadingHint />
      )}
    </StandaloneFrame>
  );
};

export const SharedRoutePage = () => {
  const { token } = useParams({ from: "/s/$token" });
  return <SharedRunView token={token} />;
};

export const SharedAliasRoutePage = () => {
  const { token } = useParams({ from: "/shared/$token" });
  return <SharedRunView token={token} />;
};
