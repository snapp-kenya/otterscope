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
  agent: "fill-agent",
  llm: "fill-primary",
  tool: "fill-tool",
  generic: "fill-muted-foreground",
};

const depthIndent = [
  "pl-0",
  "pl-[1.2rem]",
  "pl-[2.4rem]",
  "pl-[3.6rem]",
  "pl-[4.8rem]",
  "pl-[6rem]",
  "pl-[7.2rem]",
  "pl-[8.4rem]",
  "pl-[9.6rem]",
];

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
              className={cn(
                "step-name overflow-hidden text-ellipsis whitespace-nowrap",
                depthIndent[
                  Math.min(depthOf(step, byId), depthIndent.length - 1)
                ],
              )}
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
            <span className="bar-track block h-2.5 overflow-hidden rounded-[5px]">
              <svg
                className="block h-full w-full"
                viewBox="0 0 1000 10"
                preserveAspectRatio="none"
                aria-hidden="true"
              >
                <rect
                  className="fill-surface"
                  x="0"
                  y="0"
                  width="1000"
                  height="10"
                />
                <rect
                  className={cn("bar", barColor[step.kind])}
                  x={bar.left * 10}
                  y="0"
                  width={bar.width * 10}
                  height="10"
                  rx="1"
                />
              </svg>
            </span>
          </button>
        );
      })}
    </div>
  );
};
