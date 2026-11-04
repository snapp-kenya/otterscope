import type { Message, Step } from "@/api";
import { fmtCost, fmtTokens } from "@/lib/format";
import { cn } from "@/lib/utils";

const kindText: Record<Step["kind"], string> = {
  agent: "text-agent",
  llm: "text-primary",
  tool: "text-tool",
  generic: "text-muted-foreground",
};

const Messages = ({
  messages,
  out,
}: {
  messages: Message[];
  out?: boolean;
}) => (
  <>
    {messages.map((message, index) => (
      <div key={`${out ? "out" : "in"}-${index}`} className="msg mt-3">
        <span className="role mb-1 block text-[0.7rem] uppercase tracking-[0.04em] text-muted-foreground">
          {message.role}
          {out ? " (output)" : ""}
        </span>
        <pre
          className={cn(
            "m-0 rounded-md bg-surface p-2.5 text-[0.82rem] whitespace-pre-wrap break-words",
            out && "border-l-[3px] border-primary",
          )}
        >
          {message.content}
        </pre>
      </div>
    ))}
  </>
);

export const RunInspector = ({ step }: { step: Step }) => (
  <div className="inspector mt-6 rounded-lg border border-line px-4 py-3">
    <h2 className="m-0 mb-2 text-sm">
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
    </h2>
    {step.error ? (
      <p className="my-2 rounded-md bg-danger-bg px-2.5 py-1.5 text-xs text-danger">
        {step.error}
      </p>
    ) : null}
    {step.llm ? (
      <>
        <p className="text-xs text-muted-foreground">
          {step.llm.provider ? `${step.llm.provider} · ` : ""}
          {step.llm.requestModel} · {fmtTokens(step.llm.inputTokens)} in
          {step.llm.cacheReadTokens > 0
            ? ` (${fmtTokens(step.llm.cacheReadTokens)} cached)`
            : ""}{" "}
          / {fmtTokens(step.llm.outputTokens)} out
          {step.llm.reasoningTokens > 0
            ? ` (${fmtTokens(step.llm.reasoningTokens)} reasoning)`
            : ""}
          {step.llm.costUsd !== undefined
            ? ` · ${fmtCost(step.llm.costUsd)}`
            : ""}
        </p>
        <Messages messages={step.llm.inputMessages ?? []} />
        <Messages messages={step.llm.outputMessages ?? []} out />
        {!step.llm.inputMessages && !step.llm.outputMessages ? (
          <p className="mt-2 text-xs text-muted-foreground">
            No message content recorded — enable content capture in your
            instrumentation to see prompts here.
          </p>
        ) : null}
      </>
    ) : null}
    {step.tool ? (
      <>
        <p className="text-xs text-muted-foreground">
          {step.tool.name}
          {step.tool.callId ? ` · ${step.tool.callId}` : ""}
        </p>
        {step.tool.arguments ? (
          <div className="msg mt-3">
            <span className="role mb-1 block text-[0.7rem] uppercase tracking-[0.04em] text-muted-foreground">
              arguments
            </span>
            <pre className="m-0 rounded-md bg-surface p-2.5 text-[0.82rem] whitespace-pre-wrap break-words">
              {step.tool.arguments}
            </pre>
          </div>
        ) : null}
        {step.tool.result ? (
          <div className="msg mt-3">
            <span className="role mb-1 block text-[0.7rem] uppercase tracking-[0.04em] text-muted-foreground">
              result
            </span>
            <pre className="m-0 rounded-md border-l-[3px] border-primary bg-surface p-2.5 text-[0.82rem] whitespace-pre-wrap break-words">
              {step.tool.result}
            </pre>
          </div>
        ) : null}
      </>
    ) : null}
  </div>
);
