import { Globe } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { t, tf } from '@/locales';
import { shortcutKeyLabel, type ShortcutAction } from '@/stores/shortcuts';
import { bossKeyConflicts, type BossKeyConflict } from '@/utils/boss-key';

/** 老板键候选（tauri accelerator 形态；CommandOrControl 在 mac 映射 Cmd）。
 *  自定义组合后续接快捷键捕获，先给常用预设——Alt+Z 是 NVIDIA 覆盖层
 *  热键，别放进来（实测占用报错）。 */
export const BOSS_KEY_PRESETS = [
  { value: 'CommandOrControl+Alt+H', label: 'Ctrl/Cmd + Alt + H' },
  { value: 'CommandOrControl+Alt+X', label: 'Ctrl/Cmd + Alt + X' },
  { value: 'CommandOrControl+Alt+B', label: 'Ctrl/Cmd + Alt + B' },
] as const;

/** 老板键 vs 播放器快捷键冲突检测（比对逻辑在 utils/boss-key，这里负责
 *  把播放器键映射展开成通用入参）。 */
export function resolveBossKeyConflicts(
  bossKey: string,
  keys: Record<ShortcutAction, string>,
  spaceLabel: string,
): BossKeyConflict[] {
  const actions: { action: ShortcutAction; label: string; key: string }[] = [
    {
      action: 'playPause',
      label: shortcutKeyLabel(keys.playPause, spaceLabel),
      key: keys.playPause,
    },
    { action: 'seekBack', label: shortcutKeyLabel(keys.seekBack, spaceLabel), key: keys.seekBack },
    {
      action: 'seekForward',
      label: shortcutKeyLabel(keys.seekForward, spaceLabel),
      key: keys.seekForward,
    },
    { action: 'stepPrev', label: shortcutKeyLabel(keys.stepPrev, spaceLabel), key: keys.stepPrev },
    { action: 'stepNext', label: shortcutKeyLabel(keys.stepNext, spaceLabel), key: keys.stepNext },
  ];
  return bossKeyConflicts(bossKey, actions);
}

/** 老板键卡片（全局快捷键，独立于播放器快捷键卡）：应用外也生效，
 *  隐藏窗口并暂停播放。值走主设置的草稿/保存流，注册在保存后端钩子里。 */
export function BossKeyCard({
  bossKey,
  conflicts,
  onChange,
}: {
  bossKey: string;
  conflicts: BossKeyConflict[];
  onChange: (v: string) => void;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Globe className="size-4" aria-hidden />
          {t('settings.bossKeyCardTitle')}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <Label>{t('settings.bossKey')}</Label>
        <Select value={bossKey} onValueChange={onChange}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="">{t('settings.bossKeyOff')}</SelectItem>
            {BOSS_KEY_PRESETS.map((p) => (
              <SelectItem key={p.value} value={p.value}>
                {p.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {conflicts.length > 0 && (
          <p className="text-destructive text-xs">
            {tf('settings.bossKeyConflict', {
              actions: conflicts.map((c) => c.label).join('、'),
            })}
          </p>
        )}
        <p className="text-muted-foreground text-xs">{t('settings.bossKeyDesc')}</p>
        <p className="text-muted-foreground text-xs">{t('settings.bossKeyGlobalHint')}</p>
      </CardContent>
    </Card>
  );
}
