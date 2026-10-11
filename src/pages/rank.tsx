import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/rank')({
  component: lazyRouteComponent(() => import('@/pages/rank/components/rank-page'), 'RankPage'),
});
