/** 解析按钮：档案缺失/无分集时的重试出口。 */
import { useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { keys, useResolveSeries } from '@/service/queries';
import { t } from '@/locales';

/** 解析按钮（档案缺失/无分集时的出口）。解析完失效档案缓存，就地换挡。 */
export function ResolveButton({ seriesId }: { seriesId: string }) {
  const qc = useQueryClient();
  const resolve = useResolveSeries();
  return (
    <Button
      variant="outline"
      size="sm"
      className="mx-auto"
      disabled={resolve.isPending}
      onClick={() =>
        resolve.mutate(seriesId, {
          onSuccess: () => {
            void qc.invalidateQueries({ queryKey: keys.seriesEpisodes(seriesId) });
            toast.success(t('detail.resolved'));
          },
        })
      }
    >
      {t('series.resolveAgain')}
    </Button>
  );
}
