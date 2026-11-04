import { ApiError, errorMessage } from "@/api";

export const shareActionError = (
  action: "create" | "revoke",
  error: unknown,
): string => {
  if (error instanceof ApiError && error.status === 403) {
    return action === "create"
      ? "Creating a public link needs an admin token."
      : "Revoking a public link needs an admin token.";
  }
  if (error instanceof ApiError && error.status === 401) {
    return "The token was rejected — check the read/admin token.";
  }
  return action === "create"
    ? `Couldn't mint a link — ${errorMessage(error)}.`
    : `Couldn't revoke the link — ${errorMessage(error)}.`;
};
