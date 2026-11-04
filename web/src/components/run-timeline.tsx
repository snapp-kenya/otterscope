import type { Step } from "@/api";
import { fmtDuration } from "@/lib/format";
import { barPct } from "@/lib/timeline";
import { cn } from "@/lib/utils";

const kindText: Record<Step["kind"], string> = {
  agent: "text-agent",
  llm: "text-primary",
  tool: "text-tool",
  generic: "text-muted-foreground",
};

const barColor: Record<Step["kind"], string> = {
  agent: "bg-agent",
  llm: "bg-primary",
  tool: "bg-tool",
  generic: "bg-muted-foreground",
};

const depthOf = (step: Step, byId: Map<string, Step>): number => {
  let depth = 0;
  let current = step;
  while (current.parentId && byId.has(current.parentId) && depth < 32) {
    current = byId.get(current.parentId)!;
    depth += 1;
  }
  return depth;
};

export const RunTimeline = ({
  steps,
  totalMs,
  selectedId,
  onSelect,
}: {
  steps: Step[];
  totalMs: number;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) => {
  const byId = new Map(steps.map((step) => [step.id, step]));

  return (
    <div className="timeline mt-6 overflow-hidden rounded-lg border border-line">
      {steps.map((step) => {
        const bar = barPct(step.offsetMs, step.durationMs, totalMs);
        return (
          <button
            type="button"
            key={step.id}
            className={cn(
              "step grid w-full grid-cols-[minmax(240px,1fr)_70px_2fr] items-center gap-3 border-b border-line-faint px-2.5 py-1.5 text-left text-[0.82rem] last:border-b-0 hover:bg-surface-raised/70",
              selectedId === step.id && "selected bg-surface-raised/70",
            )}
            onClick={() => onSelect(step.id)}
          >
            <span
              className="step-name overflow-hidden text-ellipsis whitespace-nowrap"
              style={{ paddingLeft: `${depthOf(step, byId) * 1.2}rem` }}
            >
              <span
                className={cn(
                  "kind mr-2 inline-block rounded bg-surface-raised px-1.5 py-px text-[0.7rem]",
                  step.kind,
                  kindText[step.kind],
                )}
              >
                {step.kind}
              </span>
              {step.name}
              {step.status === "error" ? (
                <span className="ml-2 inline-block rounded-full bg-danger-bg px-1.5 text-[0.7rem] text-danger">
                  !
                </span>
              ) : null}
            </span>
            <span className="step-duration numeric text-right text-muted-foreground">
              {fmtDuration(step.durationMs)}
            </span>
            <span className="bar-track relative block h-2.5 rounded-[5px] bg-surface">
              <span
                className={cn(
                  "bar absolute top-0 h-full rounded-[5px]",
                  barColor[step.kind],
                )}
                style={{ left: `${bar.left}%`, width: `${bar.width}%` }}
              />
            </span>
          </button>
        );
      })}
    </div>
  );
};
