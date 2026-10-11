/** 无 HEVC 扩展机器的安装引导卡：说明 + 一键安装 + 暂不安装。
 *
 * 只在本机探测确认解不了 HEVC 时出现（探测为 true 或未落定都不渲染），
 * 每会话至多弹一次、可关闭。安装走后端命令（下载 → SHA-256 校验 →
 * Add-AppxPackage），成功后用返回值刷新能力缓存并由父层重试起播。
 * 手动指引与故障排查见 docs/faq.md。
 */
import { useState } from 'react';
import { Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { t, tf } from '@/locales';
import { useInstallHevcExtension } from '@/service/queries';

/** 会话内「暂不安装」记号：模块级——切剧/迷你窗重挂播放页不再重复打扰 */
let dismissedThisSession = false;

export function HevcInstallCard({
  hevcPlayable,
  onInstalled,
}: {
  hevcPlayable: boolean;
  /** 安装成功且能力重探通过后由父层触发（画面没在播时重试起播） */
  onInstalled: () => void;
}) {
  const install = useInstallHevcExtension();
  const [dismissed, setDismissed] = useState(dismissedThisSession);

  if (hevcPlayable || dismissed) return null;

  return (
    // 底部居中悬浮而不是全屏盖层：用户可能正看着 h264 降档画面，安装是
    // 质量升级的可选项，不该打断正在播的内容
    <div className="absolute inset-x-0 bottom-14 z-30 flex justify-center px-6">
      <div className="bg-popover text-popover-foreground flex w-full max-w-sm flex-col items-start gap-2 rounded-lg border p-4 shadow-lg">
        <p className="text-sm font-medium">{t('player.hevcCardTitle')}</p>
        <p className="text-muted-foreground text-xs leading-relaxed">{t('player.hevcCardDesc')}</p>
        <div className="mt-1 flex w-full gap-2">
          <Button
            size="sm"
            className="flex-1"
            disabled={install.isPending}
            onClick={() =>
              install.mutate(undefined, {
                onSuccess: (playable) => {
                  if (playable) {
                    toast.success(t('player.hevcInstallSuccess'));
                    onInstalled();
                  } else {
                    // 装了却探不到：罕见（版本过老的 WebView2 等），重启兜底
                    toast.warning(t('player.hevcStillNoPicture'));
                  }
                },
                onError: (e) => {
                  toast.error(tf('player.hevcInstallFailed', { reason: e.message }));
                },
              })
            }
          >
            {install.isPending && <Loader2 className="size-3.5 animate-spin" />}
            {install.isPending ? t('player.hevcInstalling') : t('player.hevcInstall')}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              dismissedThisSession = true;
              setDismissed(true);
            }}
          >
            {t('player.hevcLater')}
          </Button>
        </div>
      </div>
    </div>
  );
}
