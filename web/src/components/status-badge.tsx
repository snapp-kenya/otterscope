import type { RunStatus } from "@/api";
import { Badge } from "@/components/ui/badge";

export const StatusBadge = ({ status }: { status: RunStatus }) => (
  <Badge variant={status}>{status}</Badge>
);
