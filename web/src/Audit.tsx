import { useEffect, useState } from "react";
import { apiFetch, failureMessage, fmtStart, transportMessage } from "./api";
import { withBase } from "./base";

type Entry = { at: string; action: string; entity: string; detail: string };

export default function Audit() {
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let stop = false;
    const load = async () => {
      try {
        const res = await apiFetch(withBase("/api/audit?limit=500"));
        if (!res.ok) {
          // The old catch dropped this on the floor and left the null
          // sentinel meaning "loading", so a 500 or a dismissed token prompt
          // spun forever.
          const msg = await failureMessage(res);
          if (!stop) setError(msg);
          return;
        }
        const d = await res.json();
        if (!stop) {
          setEntries(d.entries ?? []);
          setError(null);
        }
      } catch (err) {
        if (!stop) setError(transportMessage(err));
      }
    };
    load();
    return () => {
      stop = true;
    };
  }, [attempt]);

  const retry = () => {
    setError(null);
    setAttempt((n) => n + 1);
  };

  if (entries === null) {
    if (error) {
      return (
        <p className="load-error" role="status">
          <span className="run-error">
            Couldn&apos;t load the audit log — {error}.
          </span>
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
      <h2 className="compare-title">Audit log</h2>
      <p className="hint">
        Every create/delete of assertions, alerts, projects, tokens, views, and
        shares — from the UI, API, or CLI.
      </p>
      {error && (
        <p className="load-error" role="status">
          <span className="run-error">
            Couldn&apos;t refresh the audit log — {error}.
          </span>
          <button className="link-btn accent" onClick={retry}>
            retry
          </button>
        </p>
      )}
      {entries.length === 0 ? (
        <p className="hint">No changes recorded yet.</p>
      ) : (
        <table className="runs">
          <thead>
            <tr>
              <th>when</th>
              <th>action</th>
              <th>entity</th>
              <th>detail</th>
            </tr>
          </thead>
          <tbody>
            {entries.map((e, i) => (
              <tr key={i}>
                <td>{fmtStart(e.at)}</td>
                <td>
                  <span className={`badge ${e.action === "delete" ? "error" : "ok"}`}>
                    {e.action}
                  </span>
                </td>
                <td>{e.entity}</td>
                <td className="truncate" title={e.detail}>
                  {e.detail}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
