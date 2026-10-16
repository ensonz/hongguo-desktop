import { useEffect, useRef } from 'react';
import { cn } from '@/lib/utils';

/**
 * 末帧定格层：把 use-frame-hold 抓下的 canvas 帧钉在画面上，新流首帧
 * 出画后交叉淡出。
 *
 * canvas 是切换提交期命令式抓取的产物（带着当时的解码画面，重建即失真），
 * 经宿主 div 挂进树；样式全走宿主的子选择器（canvas 本身不可改），
 * object-fit 与 <video> 同款（小窗裁切、大屏留黑边），定格帧与真画面
 * 逐像素对齐。渲染在带切换动画的舞台容器**外**——定格帧是静止的参考帧，
 * 不跟新内容一起滑入，否则「上一集末帧滑一下再停」比闪封面还怪。
 */
export function FrameHold({
  frame,
  hidden,
  cover,
}: {
  frame: HTMLCanvasElement;
  hidden: boolean;
  cover: boolean;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    hostRef.current?.replaceChildren(frame);
  }, [frame]);
  return (
    <div
      ref={hostRef}
      aria-hidden
      className={cn(
        'pointer-events-none absolute inset-0 z-[5] transition-opacity duration-300',
        cover ? '[&>canvas]:object-cover' : '[&>canvas]:object-contain',
        '[&>canvas]:size-full',
        hidden && 'opacity-0',
      )}
    />
  );
}
