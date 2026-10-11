import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/browse')({
  component: lazyRouteComponent(
    () => import('@/pages/series/components/browse-page'),
    'BrowsePage',
  ),
});
