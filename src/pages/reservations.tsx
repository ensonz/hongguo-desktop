import { createFileRoute, lazyRouteComponent } from '@tanstack/react-router';

export const Route = createFileRoute('/reservations')({
  component: lazyRouteComponent(
    () => import('@/pages/reservation/components/reservation-page'),
    'ReservationPage',
  ),
});
