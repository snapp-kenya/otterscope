import { Link } from "@tanstack/react-router";
import { StandaloneFrame } from "@/components/standalone-frame";

export const NotFoundPage = () => (
  <div className="mt-10 text-center">
    <p className="text-sm">Page not found.</p>
    <p className="mt-1 text-xs text-muted-foreground">
      The URL does not match a view.
    </p>
    <Link to="/" className="mt-3 inline-block text-xs text-primary hover:underline">
      ← runs
    </Link>
  </div>
);

export const NotFoundStandalone = () => (
  <StandaloneFrame tagline="not found">
    <NotFoundPage />
  </StandaloneFrame>
);
