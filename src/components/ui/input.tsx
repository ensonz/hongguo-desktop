import * as React from 'react';
import { cn } from '@/lib/utils';

function Input({ className, type, ...props }: React.ComponentProps<'input'>) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        'file:text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground border-input flex h-9 w-full min-w-0 rounded-md border bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm',
        'focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]',
        'aria-invalid:ring-destructive/20 aria-invalid:border-destructive',
        // 滑条走原生渲染：文本框的边框与左右内边距会让 Chromium 把轨道
        // 画满全宽、滑块却只能在内容盒里走——到最大值滑块右侧仍留一段
        // 死区（值已满、条不满）。range 一律零边框零内边距。
        type === 'range' && 'h-auto cursor-pointer border-0 p-0 shadow-none',
        className,
      )}
      {...props}
    />
  );
}

export { Input };
