import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/player')({
  component: lazyRouteComponent(
    () => import('@/features/player/components/player-page'),
    'PlayerPage',
  ),
});
