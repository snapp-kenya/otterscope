import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { parseCompareSearch } from "@/lib/compare";
import { parseRunsSearch } from "@/lib/filters";
import { BASE_PATH } from "@/lib/base";
import { RootLayout } from "@/components/root-layout";
import { NotFoundPage, NotFoundStandalone } from "@/components/not-found";
import { RunsPage } from "@/routes/runs";
import { RunDetailPage } from "@/routes/run-detail";
import { ComparePage } from "@/routes/compare";
import { AlertsPage } from "@/routes/alerts";
import { AuditPage } from "@/routes/audit";
import {
  SharedAliasRoutePage,
  SharedRoutePage,
} from "@/routes/shared";

const rootRoute = createRootRoute({
  component: RootLayout,
  notFoundComponent: NotFoundStandalone,
});

const runsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  validateSearch: parseRunsSearch,
  component: RunsPage,
});

const runDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/runs/$runId",
  component: RunDetailPage,
});

const compareRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/compare",
  validateSearch: parseCompareSearch,
  component: ComparePage,
});

const alertsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/alerts",
  component: AlertsPage,
});

const auditRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/audit",
  component: AuditPage,
});

const notFoundRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "$",
  component: NotFoundPage,
});

const sharedRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/s/$token",
  component: SharedRoutePage,
});

const sharedAliasRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/shared/$token",
  component: SharedAliasRoutePage,
});

const routeTree = rootRoute.addChildren([
  runsRoute,
  runDetailRoute,
  compareRoute,
  alertsRoute,
  auditRoute,
  notFoundRoute,
  sharedRoute,
  sharedAliasRoute,
]);

export const router = createRouter({
  routeTree,
  basepath: BASE_PATH || "/",
  defaultPreload: false,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
