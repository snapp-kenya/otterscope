import { describe, expect, it } from "bun:test";
import { ApiError } from "@/api";
import { shareActionError } from "@/lib/shareActions";

describe("shareActionError", () => {
  it("names the admin requirement instead of a generic failure", () => {
    const error = new ApiError("403 Forbidden: admin token required", 403);
    expect(shareActionError("create", error)).toBe(
      "Creating a public link needs an admin token.",
    );
    expect(shareActionError("revoke", error)).toBe(
      "Revoking a public link needs an admin token.",
    );
  });

  it("explains a rejected token without a generic failure", () => {
    const error = new ApiError("401 Unauthorized", 401);
    expect(shareActionError("create", error)).toBe(
      "The token was rejected — check the read/admin token.",
    );
  });

  it("keeps the server detail for other failures", () => {
    expect(
      shareActionError("create", new ApiError("500: query failed", 500)),
    ).toContain("query failed");
    expect(
      shareActionError("revoke", new TypeError("Failed to fetch")),
    ).toMatch(/could not reach the server/i);
  });
});
