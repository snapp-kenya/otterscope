import { useEffect, useRef, useState } from "react";

export const useDebouncedValue = <T>(value: T, delayMs: number): T => {
  const [debounced, setDebounced] = useState(value);
  const latest = useRef(value);

  useEffect(() => {
    latest.current = value;
  }, [value]);

  const serialized = JSON.stringify(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(latest.current), delayMs);
    return () => clearTimeout(timer);
  }, [serialized, delayMs]);

  return debounced;
};
