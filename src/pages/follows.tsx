import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/follows')({
  component: lazyRouteComponent(
    () => import('@/pages/follows/components/follows-page'),
    'FollowsPage',
  ),
});
