import { useEffect, useState } from "react";
import { apiFetch, failureMessage, transportMessage } from "./api";
import { withBase } from "./base";
import { withFilterDefaults, type FilterState } from "./filterState";

type View = { id: number; name: string; params: Partial<FilterState> };

// SavedViews renders named filter presets: click to apply, save the current
// filters under a name, or delete one.
export default function SavedViews({
  current,
  onApply,
}: {
  current: FilterState;
  onApply: (f: FilterState) => void;
}) {
  // null is "not read yet", never "no views" — the empty list is a real answer
  // and the two must not render the same way.
  const [views, setViews] = useState<View[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  // A 500 here used to be swallowed by `.catch(() => {})`, leaving `views` as
  // the null sentinel; the next render called `.map` on it and took the whole
  // run list down. A failure is now a settled state with a retry, and the
  // body is checked before it reaches state.
  useEffect(() => {
    let stop = false;
    const load = async () => {
      try {
        const res = await apiFetch(withBase("/api/views"));
        if (!res.ok) {
          const msg = await failureMessage(res);
          if (!stop) setLoadError(msg);
          return;
        }
        const d = await res.json();
        if (stop) return;
        if (!Array.isArray(d?.views)) {
          setLoadError("the server did not return a list of views");
          return;
        }
        setViews(d.views as View[]);
        setLoadError(null);
      } catch (err) {
        if (!stop) setLoadError(transportMessage(err));
      }
    };
    load();
    return () => {
      stop = true;
    };
  }, [attempt]);

  // Reload through the effect so a successful write and a retry take the same
  // path.
  const reload = () => setAttempt((n) => n + 1);

  const save = async () => {
    const name = window.prompt("Name this view (e.g. 'prod errors 24h')");
    if (!name) return;
    setActionError(null);
    try {
      const res = await apiFetch(withBase("/api/views"), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name, params: current }),
      });
      if (!res.ok) {
        setActionError(`Couldn't save view — ${await failureMessage(res)}.`);
        return;
      }
    } catch (err) {
      setActionError(`Couldn't save view — ${transportMessage(err)}.`);
      return;
    }
    reload();
  };

  const remove = async (id: number) => {
    setActionError(null);
    try {
      const res = await apiFetch(withBase(`/api/views/${id}`), {
        method: "DELETE",
      });
      if (!res.ok) {
        // A refused delete that says nothing reads as success; the chip stays
        // on screen and the reader has no idea why.
        setActionError(`Couldn't delete view — ${await failureMessage(res)}.`);
        return;
      }
    } catch (err) {
      setActionError(`Couldn't delete view — ${transportMessage(err)}.`);
      return;
    }
    reload();
  };

  return (
    <div className="views">
      {loadError && (
        <span className="run-error" role="status">
          {views === null
            ? "Couldn't read saved views — "
            : "Couldn't refresh saved views — "}
          {loadError}.
        </span>
      )}
      {loadError && (
        <button
          className="link-btn accent"
          onClick={() => {
            setLoadError(null);
            setActionError(null);
            reload();
          }}
        >
          retry
        </button>
      )}
      {actionError && (
        <span className="run-error" role="status">
          {actionError}
        </span>
      )}
      {views?.map((v) => (
        <span key={v.id} className="view-chip">
          <button
            className="view-apply"
            onClick={() => onApply(withFilterDefaults(v.params))}
          >
            {v.name}
          </button>
          <button
            className="view-del"
            title="delete view"
            onClick={() => remove(v.id)}
          >
            ×
          </button>
        </span>
      ))}
      <button className="view-save" onClick={save}>
        + save view
      </button>
    </div>
  );
}
