import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/merge')({
  component: lazyRouteComponent(() => import('@/pages/merge/components/merge-page'), 'MergePage'),
});
