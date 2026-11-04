import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  errorMessage,
  mintShare,
  revokeShare,
  type ShareMint,
} from "@/api";
import { fmtStart } from "@/lib/format";
import { queryKeys, sharesQuery } from "@/lib/queries";
import { Button } from "@/components/ui/button";
import { ErrorNotice } from "@/components/error-notice";

export const SharesPanel = ({ runId }: { runId: string }) => {
  const queryClient = useQueryClient();
  const sharesResult = useQuery(sharesQuery(runId));
  const [minted, setMinted] = useState<ShareMint | null>(null);
  const [copied, setCopied] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const mint = useMutation({
    mutationFn: () => mintShare(runId),
    onSuccess: async (share) => {
      setMinted({
        ...share,
        url: window.location.origin + share.url,
      });
      setCopied(false);
      setActionError(null);
      await queryClient.invalidateQueries({
        queryKey: queryKeys.shares(runId),
      });
    },
    onError: (error) =>
      setActionError(`Couldn't mint a link — ${errorMessage(error)}.`),
  });

  const revoke = useMutation({
    mutationFn: async (token: string) => {
      await revokeShare(token);
      return token;
    },
    onSuccess: async (token) => {
      if (minted?.token === token) setMinted(null);
      setActionError(null);
      await queryClient.invalidateQueries({
        queryKey: queryKeys.shares(runId),
      });
    },
    onError: (error) =>
      setActionError(`Couldn't revoke the link — ${errorMessage(error)}.`),
  });

  const copy = async () => {
    if (!minted) return;
    try {
      await navigator.clipboard.writeText(minted.url);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  const shares = sharesResult.data ?? [];

  return (
    <div className="mt-3 rounded-lg border border-line bg-surface px-3 py-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          onClick={() => mint.mutate()}
          disabled={mint.isPending}
        >
          {mint.isPending ? "creating…" : "create public link"}
        </Button>
        <span className="text-xs text-muted-foreground">
          Read-only, one run, no other data.
        </span>
      </div>

      {actionError ? (
        <p className="mt-2 text-xs text-danger" role="status">
          {actionError}
        </p>
      ) : null}

      {minted ? (
        <p className="share-link mt-2 flex flex-wrap items-center gap-2 break-all rounded-md border border-line bg-background px-2 py-1.5 text-xs">
          <a
            href={minted.url}
            target="_blank"
            rel="noreferrer"
            className="text-primary hover:underline"
          >
            {minted.url}
          </a>
          <Button variant="link" size="sm" onClick={copy}>
            {copied ? "copied" : "copy"}
          </Button>
        </p>
      ) : null}

      {sharesResult.isError ? (
        <ErrorNotice onRetry={() => void sharesResult.refetch()}>
          Couldn't read this run's links —{" "}
          {errorMessage(sharesResult.error)}.
        </ErrorNotice>
      ) : null}

      {sharesResult.isPending ? (
        <p className="mt-2 text-xs text-muted-foreground">loading links…</p>
      ) : null}

      {!sharesResult.isPending && !sharesResult.isError && shares.length === 0 ? (
        <p className="mt-2 text-xs text-muted-foreground">
          No public links yet.
        </p>
      ) : null}

      {shares.length > 0 ? (
        <ul className="mt-2 space-y-1">
          {shares.map((share) => (
            <li
              key={share.token}
              className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground"
            >
              <code className="numeric text-foreground/80">
                {share.token.slice(0, 10)}…
              </code>
              <span>created {fmtStart(share.createdAt)}</span>
              <Button
                variant="destructive"
                size="sm"
                className="ml-auto"
                onClick={() => revoke.mutate(share.token)}
                disabled={revoke.isPending}
              >
                revoke
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
};
