import { useEffect, useState } from "react";
import {
  type RunDetail as Detail,
  apiFetch,
  failureMessage,
  transportMessage,
} from "./api";
import { withBase } from "./base";
import { useLiveTick } from "./live";
import RunView from "./RunView";

const POLL_MS = 2000;

export default function RunDetail({
  id,
  navigate,
}: {
  id: string;
  navigate: (path: string) => void;
}) {
  const [detail, setDetail] = useState<Detail | null>(null);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [shareUrl, setShareUrl] = useState<string | null>(null);
  const tick = useLiveTick();

  // A different run is a different page: drop the previous one rather than
  // showing its steps under the new id while the first fetch is in flight.
  useEffect(() => {
    setDetail(null);
    setMissing(false);
    setError(null);
    setShareUrl(null);
  }, [id]);

  useEffect(() => {
    let stop = false;
    const load = async () => {
      try {
        const res = await apiFetch(withBase(`/api/runs/${id}`));
        if (res.status === 404) {
          if (!stop) {
            setMissing(true);
            setError(null);
          }
          return;
        }
        if (!res.ok) {
          // Settled, not slow. Without this the poll swallowed the failure
          // and the page said "loading…" for as long as it stayed open.
          const msg = await failureMessage(res);
          if (!stop) setError(msg);
          return;
        }
        const data = await res.json();
        if (!stop) {
          setDetail(data);
          setError(null);
        }
      } catch (err) {
        if (!stop) setError(transportMessage(err));
      }
    };
    load();
    const timer = setInterval(load, POLL_MS);
    return () => {
      stop = true;
      clearInterval(timer);
    };
  }, [id, attempt, tick]);

  const retry = () => {
    setError(null);
    setMissing(false);
    setDetail(null);
    setAttempt((n) => n + 1);
  };

  const share = async () => {
    const res = await apiFetch(withBase(`/api/runs/${id}/share`), { method: "POST" });
    if (res.ok) {
      const { url } = await res.json();
      setShareUrl(window.location.origin + url);
    }
  };

  if (missing) return <p className="hint">Run not found.</p>;
  if (!detail) {
    if (error) {
      return (
        <p className="load-error" role="status">
          <span className="run-error">Couldn&apos;t load this run — {error}.</span>
          <button className="link-btn accent" onClick={retry}>
            retry
          </button>
        </p>
      );
    }
    return <p className="hint">loading…</p>;
  }

  return (
    <div>
      <p className="run-toolbar">
        <a
          href={withBase("/")}
          onClick={(e) => {
            e.preventDefault();
            navigate("/");
          }}
        >
          ← runs
        </a>
        <button className="link-btn accent" onClick={share}>
          share
        </button>
      </p>
      {shareUrl && (
        <p className="share-link">
          Public read-only link:{" "}
          <a href={shareUrl} target="_blank" rel="noreferrer">
            {shareUrl}
          </a>
        </p>
      )}
      {/* A failed poll keeps the last good render: stale data plus an honest
          note beats replacing the run with an error page mid-read. */}
      {error && (
        <p className="load-error" role="status">
          <span className="run-error">
            This run may be out of date — {error}.
          </span>
          <button className="link-btn accent" onClick={retry}>
            retry
          </button>
        </p>
      )}
      <RunView detail={detail} />
    </div>
  );
}
