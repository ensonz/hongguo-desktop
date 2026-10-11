import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/storage')({
  component: lazyRouteComponent(
    () => import('@/pages/storage/components/storage-page'),
    'StoragePage',
  ),
});
