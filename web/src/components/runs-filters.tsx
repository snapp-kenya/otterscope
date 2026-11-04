import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { FilterState } from "@/lib/filters";

const ANY_STATUS = "any-status";
const ALL_TIME = "all-time";

export const RunsFilters = ({
  filters,
  onChange,
}: {
  filters: FilterState;
  onChange: (next: FilterState) => void;
}) => {
  const update = (patch: Partial<FilterState>) =>
    onChange({ ...filters, ...patch });

  return (
    <div className="flex flex-1 flex-wrap items-center gap-2">
      <Input
        className="min-w-[14rem] flex-1"
        placeholder="search messages & tool i/o…"
        aria-label="search runs"
        value={filters.q}
        onChange={(event) => update({ q: event.target.value })}
      />
      <Select
        value={filters.status || ANY_STATUS}
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
          <SelectItem value="running">running</SelectItem>
        </SelectContent>
      </Select>
      <Select
        value={filters.range || ALL_TIME}
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
        value={filters.model}
        onChange={(event) => update({ model: event.target.value })}
      />
      <Input
        className="w-[8.5rem]"
        placeholder="project…"
        aria-label="project filter"
        value={filters.project}
        onChange={(event) => update({ project: event.target.value })}
      />
      <Input
        className="w-[8.5rem]"
        placeholder="service…"
        aria-label="service filter"
        value={filters.service}
        onChange={(event) => update({ service: event.target.value })}
      />
      <Input
        className="w-[8.5rem]"
        placeholder="prompt…"
        aria-label="prompt filter"
        value={filters.prompt}
        onChange={(event) => update({ prompt: event.target.value })}
      />
    </div>
  );
};
