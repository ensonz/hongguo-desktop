import { useCallback } from 'react';
import { useNavigate } from '@tanstack/react-router';
import { toast } from 'sonner';
import { useUiStore } from '@/stores/ui';
import { app as appApi } from '@/service/commands';

/**
 * 小屏播放（对齐 hgplayer ng()/Op()）：同一窗口缩成 480×270 落到屏幕
 * 右下角，侧栏隐藏、控件换紧凑条——video 元素原地不动，播放零中断
 * （不暂停、不落库、不换页）。
 */

/** 本次小屏是否从信息流进入（onWheelStep 在场）：退出要**导航回首页**——
 *  /player 是纯播放页（无顶栏 tab、滚轮=切集），留在那里就是「切回正常
 *  模式丢了顶部 tab + 滚轮换不了剧」（用户实测回归）。模块级：home 与
 *  /player 是两个 hook 实例，路由切换后只有模块作用域跨得过去。 */
let enteredFromFeed = false;
export function useMiniWindow(params: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  /** 信息流上下文进小屏前强落一次进度，/player 挂载后凭 resumeAt 接上 */
  persist: (time: number, force?: boolean) => void;
  onWheelStep?: (dir: 1 | -1) => void;
  /** 进出小屏都要清悬停态：组件卸载不触发 mouseleave，残留会让控制条常显不收 */
  setControlsHovered: (hovered: boolean) => void;
  setCommentPanelOpen: (open: boolean) => void;
  setDanmakuPanelOpen: (open: boolean) => void;
  setVolumeOpen: (open: boolean) => void;
  setSeriesPanelOpen: (open: boolean) => void;
}) {
  const {
    videoRef,
    persist,
    onWheelStep,
    setControlsHovered,
    setCommentPanelOpen,
    setDanmakuPanelOpen,
    setVolumeOpen,
    setSeriesPanelOpen,
  } = params;
  const navigate = useNavigate();
  const miniScreen = useUiStore((s) => s.miniScreen);
  const setMiniScreen = useUiStore((s) => s.setMiniScreen);

  const enterMini = useCallback(() => {
    // 大屏的浮层面板带不进 480×270 的小窗：进小屏前一并收掉
    setCommentPanelOpen(false);
    setDanmakuPanelOpen(false);
    setVolumeOpen(false);
    setSeriesPanelOpen(false);
    // 悬停态跟着旧控制栏一起卸载：组件卸载不触发 mouseleave，残留 true 会让
    // 小屏里鼠标已经离开画面、控制条却常显不收
    setControlsHovered(false);
    setMiniScreen(true);
    void appApi.enterMiniScreen().catch((e: Error) => toast.error(e.message));
    // 信息流上下文（首页沉浸流内嵌本组件）：小窗里只装播放器——先强落
    // 一次进度再跳纯播放页，/player 挂载后凭 resumeAt 精准接上
    if (onWheelStep) {
      enteredFromFeed = true;
      const video = videoRef.current;
      if (video) persist(video.currentTime, true);
      void navigate({ to: '/player' });
    }
  }, [
    setMiniScreen,
    setCommentPanelOpen,
    setDanmakuPanelOpen,
    setVolumeOpen,
    setSeriesPanelOpen,
    setControlsHovered,
    onWheelStep,
    navigate,
    persist,
    videoRef,
  ]);

  /** 退出小屏：恢复窗口几何；从信息流进来的**导航回首页**——顶部 tab 与
   *  「滚轮换剧」都长在首页沉浸流上，留 /player 就是两个回归（见文件头）。
   *  回首页不换片：在播剧由 pin 机制钉住、游标对回它（PlayerView 以
   *  resumeAt 接上，与进小屏时的强落对称）。 */
  const exitMini = useCallback(() => {
    setMiniScreen(false);
    setControlsHovered(false); // 同 enterMini：悬停态别跨大小屏残留
    void appApi.exitMiniScreen().catch(() => undefined);
    if (enteredFromFeed) {
      enteredFromFeed = false;
      void navigate({ to: '/' });
    }
  }, [navigate, setMiniScreen, setControlsHovered]);

  /** 结束播放：退出小屏并回首页（小屏里唯一的「关掉」出口）。 */
  const stopMini = useCallback(() => {
    const video = videoRef.current;
    if (video && !video.paused) {
      video.pause(); // onPause 里会强制落一次进度
    }
    enteredFromFeed = false;
    setMiniScreen(false);
    setControlsHovered(false); // 同 enterMini：悬停态别跨大小屏残留
    void appApi.exitMiniScreen().catch(() => undefined);
    void navigate({ to: '/' });
  }, [navigate, setMiniScreen, setControlsHovered, videoRef]);

  return { miniScreen, enterMini, exitMini, stopMini };
}
