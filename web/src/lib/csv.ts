import {
  ApiError,
  csvTruncationNotice,
  endpoints,
  failureMessage,
  request,
} from "@/api";
import { filtersToQuery, type FilterState } from "@/lib/filters";

export const exportRunsCsv = async (
  filters: FilterState,
): Promise<string | null> => {
  const res = await request(endpoints.runsCsv(filtersToQuery(filters)));
  if (!res.ok) throw new ApiError(await failureMessage(res), res.status);
  const notice = csvTruncationNotice(res);
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = "otterscope-runs.csv";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
  return notice;
};
