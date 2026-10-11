import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/settings')({
  component: lazyRouteComponent(
    () => import('@/pages/settings/components/settings-page'),
    'SettingsPage',
  ),
});
