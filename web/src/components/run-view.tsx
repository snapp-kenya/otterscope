import { useMemo, useState } from "react";
import type { RunDetail } from "@/api";
import { fmtCost, fmtDuration, fmtStart, fmtTokens } from "@/lib/format";
import { Badge } from "@/components/ui/badge";
import { StatusBadge } from "@/components/status-badge";
import { RunInspector } from "@/components/run-inspector";
import { RunTimeline } from "@/components/run-timeline";

export const RunView = ({ detail }: { detail: RunDetail }) => {
  const { run, steps } = detail;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const byId = useMemo(
    () => new Map(steps.map((step) => [step.id, step])),
    [steps],
  );
  const selected = selectedId ? (byId.get(selectedId) ?? null) : null;
  const assertions = detail.assertionResults ?? [];

  return (
    <div>
      <div className="run-head mt-4 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <StatusBadge status={run.status} />
        <strong className="text-sm">
          {run.service || run.id.slice(0, 12)}
        </strong>
        {run.agentName ? (
          <span className="text-xs text-muted-foreground">
            / {run.agentName}
          </span>
        ) : null}
        <span className="stat text-xs text-muted-foreground">
          {fmtStart(run.start)}
        </span>
        <span className="stat numeric text-xs text-muted-foreground">
          {fmtDuration(run.durationMs)}
        </span>
        <span className="stat numeric text-xs text-muted-foreground">
          {fmtTokens(run.inputTokens)} in / {fmtTokens(run.outputTokens)} out
        </span>
        <span className="stat numeric text-xs text-muted-foreground">
          {run.llmCalls} llm · {run.toolCalls} tools
        </span>
        {run.costUsd !== undefined ? (
          <span className="stat numeric text-xs text-muted-foreground">
            {fmtCost(run.costUsd, run.costPartial)}
          </span>
        ) : null}
      </div>

      {run.error ? (
        <p className="run-error mt-3 rounded-md bg-danger-bg px-3 py-2 text-xs text-danger">
          {run.error}
        </p>
      ) : null}

      {assertions.length > 0 ? (
        <div className="asserts mt-3 flex flex-wrap gap-2">
          {assertions.map((assertion) => (
            <Badge
              key={assertion.assertionId}
              variant={assertion.pass ? "ok" : "error"}
              title={assertion.detail || assertion.type}
              className="px-2.5 py-0.5 text-[0.78rem]"
            >
              {assertion.pass ? "✓" : "✗"} {assertion.name}
            </Badge>
          ))}
        </div>
      ) : null}

      {steps.length > 0 ? (
        <RunTimeline
          steps={steps}
          totalMs={run.durationMs}
          selectedId={selectedId}
          onSelect={(id) =>
            setSelectedId(selectedId === id ? null : id)
          }
        />
      ) : (
        <p className="mt-6 text-sm text-muted-foreground">
          No steps recorded for this run.
        </p>
      )}

      {selected ? <RunInspector step={selected} /> : null}
    </div>
  );
};
