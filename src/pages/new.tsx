import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/new')({
  component: lazyRouteComponent(
    () => import('@/pages/new-drama/components/new-drama-page'),
    'NewDramaPage',
  ),
});
