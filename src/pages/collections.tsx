import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/collections')({
  component: lazyRouteComponent(
    () => import('@/pages/collection/components/collection-page'),
    'CollectionPage',
  ),
});
