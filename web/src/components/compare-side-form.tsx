import { runStatuses } from "@/api";
import type { CompareSide } from "@/lib/compare";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";

const ANY_STATUS = "any-status";
const ALL_TIME = "all-time";

export const CompareSideForm = ({
  side,
  onChange,
}: {
  side: CompareSide;
  onChange: (next: CompareSide) => void;
}) => {
  const update = (patch: Partial<CompareSide>) =>
    onChange({ ...side, ...patch });

  return (
    <div className="mt-2 flex flex-wrap gap-2">
      <Select
        className="w-[7.5rem]"
        aria-label="status filter"
        value={side.status || ANY_STATUS}
        onValueChange={(value) =>
          update({ status: value === ANY_STATUS ? "" : value })
        }
      >
        <option value={ANY_STATUS}>any status</option>
        {side.status && !runStatuses.some((s) => s === side.status) ? (
          <option value={side.status}>{side.status}</option>
        ) : null}
        {runStatuses.map((status) => (
          <option key={status} value={status}>
            {status}
          </option>
        ))}
      </Select>
      <Select
        className="w-[7.5rem]"
        aria-label="time range"
        value={side.range || ALL_TIME}
        onValueChange={(value) =>
          update({ range: value === ALL_TIME ? "" : value })
        }
      >
        <option value={ALL_TIME}>all time</option>
        <option value="1h">last hour</option>
        <option value="24h">last 24h</option>
        <option value="7d">last 7 days</option>
      </Select>
      <Input
        className="w-[8.5rem]"
        placeholder="model…"
        aria-label="model filter"
        value={side.model}
        onChange={(event) => update({ model: event.target.value })}
      />
      <Input
        className="w-[8.5rem]"
        placeholder="service…"
        aria-label="service filter"
        value={side.service}
        onChange={(event) => update({ service: event.target.value })}
      />
      <Input
        className="w-[8.5rem]"
        placeholder="prompt version…"
        aria-label="prompt filter"
        value={side.prompt}
        onChange={(event) => update({ prompt: event.target.value })}
      />
    </div>
  );
};
