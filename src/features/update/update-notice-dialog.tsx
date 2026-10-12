import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { t, tf } from '@/locales';
import { useUpdate } from './update-context';

/** 手动检查更新的「已是最新版本」回执弹层：用户主动点了检查，就得给个明确
 *  结果，不能像自动检查那样静默。有新版的走 UpdateDialog——两种结果各是各的
 *  形态，不塞进同一个弹层。 */
export function UpdateNoticeDialog() {
  const { upToDateOpen, setUpToDateOpen, version } = useUpdate();

  return (
    <AlertDialog open={upToDateOpen} onOpenChange={setUpToDateOpen}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t('update.upToDate')}</AlertDialogTitle>
          <AlertDialogDescription>{tf('update.upToDateDesc', { version })}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogAction>{t('common.confirm')}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
