import type { CompareSide } from "@/lib/compare";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

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
        value={side.status || ANY_STATUS}
        onValueChange={(value) =>
          update({ status: value === ANY_STATUS ? "" : value })
        }
      >
        <SelectTrigger className="w-[7.5rem]" aria-label="status filter">
          <SelectValue placeholder="any status" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ANY_STATUS}>any status</SelectItem>
          <SelectItem value="ok">ok</SelectItem>
          <SelectItem value="error">error</SelectItem>
        </SelectContent>
      </Select>
      <Select
        value={side.range || ALL_TIME}
        onValueChange={(value) =>
          update({ range: value === ALL_TIME ? "" : value })
        }
      >
        <SelectTrigger className="w-[7.5rem]" aria-label="time range">
          <SelectValue placeholder="all time" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL_TIME}>all time</SelectItem>
          <SelectItem value="1h">last hour</SelectItem>
          <SelectItem value="24h">last 24h</SelectItem>
          <SelectItem value="7d">last 7 days</SelectItem>
        </SelectContent>
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
