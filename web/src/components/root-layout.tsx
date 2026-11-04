import { Outlet, useRouterState } from "@tanstack/react-router";
import { AppShell } from "@/components/app-shell";

export const RootLayout = () => {
  const standalone = useRouterState({
    select: (state) =>
      state.matches.some(
        (match) =>
          match.routeId === "/s/$token" || match.routeId === "/shared/$token",
      ),
  });
  return standalone ? <Outlet /> : <AppShell />;
};
