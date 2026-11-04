import { Link, Outlet } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useLiveRuns } from "@/lib/live";
import { healthQuery } from "@/lib/queries";

const navLink =
  "text-xs text-primary/80 hover:text-primary data-[status=active]:text-primary";

export const AppShell = () => {
  useLiveRuns();
  const health = useQuery(healthQuery());

  return (
    <div className="mx-auto max-w-[60rem] px-6 py-8">
      <header className="flex flex-wrap items-baseline gap-x-4 gap-y-1 border-b border-line pb-3">
        <h1 className="m-0 text-[1.4rem] leading-tight">
          <Link to="/" className="text-foreground hover:text-foreground">
            🦦 Otterscope
          </Link>
        </h1>
        <span className="text-xs text-muted-foreground">
          {health.data ? `v${health.data.version}` : "…"}
        </span>
        <nav className="ml-auto flex gap-4">
          <Link to="/compare" className={navLink}>
            compare
          </Link>
          <Link to="/alerts" className={navLink}>
            alerts
          </Link>
          <Link to="/audit" className={navLink}>
            audit
          </Link>
        </nav>
      </header>
      <main className="pt-3">
        <Outlet />
      </main>
    </div>
  );
};
