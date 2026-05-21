import { createHashHistory, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";

import { AppShell } from "@/app/AppShell";
import { DashboardPage } from "@/features/dashboard/DashboardPage";
import { ProvidersPage } from "@/features/providers/ProvidersPage";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { UsagePage } from "@/features/usage/UsagePage";

const rootRoute = createRootRoute({
  component: AppShell,
});

const dashboardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DashboardPage,
});

const providersRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/providers",
  component: ProvidersPage,
});

const usageRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/usage",
  component: UsagePage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const routeTree = rootRoute.addChildren([
  dashboardRoute,
  providersRoute,
  usageRoute,
  settingsRoute,
]);

export const router = createRouter({
  history: createHashHistory(),
  routeTree,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
