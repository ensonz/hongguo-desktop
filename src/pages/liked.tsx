import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/liked')({
  component: lazyRouteComponent(() => import('@/pages/liked/components/liked-page'), 'LikedPage'),
});
