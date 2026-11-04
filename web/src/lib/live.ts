import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { readToken } from "@/api";
import { withBase } from "@/lib/base";
import { queryKeys } from "@/lib/queries";

const RECONNECT_MS = 5000;
const SAFETY_REFRESH_MS = 15000;

export const useLiveRuns = (): void => {
  const queryClient = useQueryClient();

  useEffect(() => {
    let source: EventSource | null = null;
    let stopped = false;
    let reconnect: ReturnType<typeof setTimeout> | null = null;

    const refresh = () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.runsRoot });
      void queryClient.invalidateQueries({ queryKey: queryKeys.runRoot });
    };

    const connect = () => {
      if (stopped) return;
      const token = readToken();
      const url = withBase("/api/stream") + (token ? `?token=${encodeURIComponent(token)}` : "");
      source = new EventSource(url);
      source.addEventListener("runs", refresh);
      source.onerror = () => {
        source?.close();
        source = null;
        if (!stopped) reconnect = setTimeout(connect, RECONNECT_MS);
      };
    };

    connect();
    const safety = setInterval(refresh, SAFETY_REFRESH_MS);

    return () => {
      stopped = true;
      clearInterval(safety);
      if (reconnect) clearTimeout(reconnect);
      source?.close();
    };
  }, [queryClient]);
};
