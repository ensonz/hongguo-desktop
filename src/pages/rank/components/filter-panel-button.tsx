/** 榜单筛选入口按钮：弹出面板按子榜自带 panel 行渲染，选项单选。 */
import { useState } from 'react';
import { ChevronDown, SlidersHorizontal, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { t } from '@/locales';
import { cn } from '@/lib/utils';

/** 「总榜 ▾」筛选按钮 + 弹出面板（row_name 分行，选项单选整组替换）。
 *  `placeholder` 覆盖未选中时的按钮文案（月榜入口用「月榜」）。 */
export function FilterPanelButton({
  rows,
  value,
  onPick,
  placeholder,
}: {
  rows: { name: string; items: { id: string; name: string }[] }[];
  value: string;
  onPick: (id: string) => void;
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const selectedName = rows
    .flatMap((r) => r.items)
    .find((it) => it.id === value && it.id !== '')?.name;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="outline" size="sm">
          <SlidersHorizontal className="size-4" aria-hidden />
          {selectedName ??
            placeholder ??
            rows[0]?.items.find((it) => it.id === '')?.name ??
            t('rank.filter.all')}
          <ChevronDown
            className={cn('size-4 transition-transform', open && 'rotate-180')}
            aria-hidden
          />
        </Button>
      </PopoverTrigger>

      <PopoverContent
        align="end"
        /* w-96 长面板：限高滚动，点选项即收起由各选项自行处理 */
        className="max-h-[60vh] w-96 overflow-y-auto rounded-lg p-3"
      >
        <div className="mb-2 flex items-center justify-between">
          <p className="text-sm font-medium">{t('rank.filter.title')}</p>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setOpen(false)}
            aria-label={t('common.close')}
          >
            <X className="size-4" aria-hidden />
          </button>
        </div>
        {value !== '' && (
          <div className="mb-2 flex justify-end">
            <Button variant="ghost" size="sm" onClick={() => onPick('')}>
              {t('rank.filter.reset')}
            </Button>
          </div>
        )}
        <div className="flex flex-col gap-3">
          {rows.map((row) => (
            <div key={row.name} className="flex flex-col gap-1.5">
              <p className="text-muted-foreground text-xs">{row.name}</p>
              <div className="flex flex-wrap gap-1.5">
                {row.items.map((it) => (
                  <button
                    key={it.id === '' ? '__all__' : it.id}
                    type="button"
                    onClick={() => {
                      onPick(it.id);
                      setOpen(false);
                    }}
                    className={cn(
                      'rounded-full border px-2.5 py-1 text-xs transition-colors',
                      it.id === value
                        ? 'border-primary bg-primary text-primary-foreground'
                        : 'hover:bg-accent hover:text-accent-foreground',
                    )}
                  >
                    {it.name}
                  </button>
                ))}
              </div>
            </div>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}
