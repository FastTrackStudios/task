// First import on purpose: wraps globalThis.WebSocket and installs the
// vox logger bridge before any socket/session exists. `?debug=vox`
// shows the overlay; `window.voxPerf.table()` dumps the ring buffer.
import { installTelemetry } from "./lib/telemetry";
installTelemetry();

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";

import { AccountProvider } from "./lib/auth";
import { OrgProvider } from "./lib/orgs";
import { createAppRouter } from "./router";
import "./styles/index.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // The lab talks to a local dev server; fail fast and visibly
      // rather than retrying into a skeleton forever.
      retry: 1,
      refetchOnWindowFocus: false,
      staleTime: 10_000,
    },
  },
});

const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <OrgProvider>
        <AccountProvider>
          <RouterProvider router={router} />
        </AccountProvider>
      </OrgProvider>
    </QueryClientProvider>
  </StrictMode>,
);
