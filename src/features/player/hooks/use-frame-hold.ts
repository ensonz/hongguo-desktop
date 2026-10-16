import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';

/** 切换类型：同剧切集走末帧定格过渡，切剧走封面接管。 */
export type SwitchKind = 'episode' | 'series';

/** 定格帧淡出时长（FrameHold 的 transition 是 300ms，出画后再留一拍清掉）。 */
const HOLD_FADE_MS = 350;

/**
 * 切集闪封面的根治：同剧切集的间隙用「上一集正在显示的那一帧」顶住，
 * 直到新集首帧出画——封面占位只留给切剧与初次起播（那才是它对的内容）。
 *
 * 抓帧必须赶在 episodeKey 变化的那次提交之后、新流地址到达之前
 * （useLayoutEffect）：此刻取流还没回来、旧 `<video>` 仍挂在 DOM 里，是
 * 唯一拿得到「正在显示的帧」的时机；流地址一交换元素就被 key 换掉，
 * 再抓只剩黑。自定义协议的流与页面不同源，画进 canvas 会污染画布——
 * 污染只禁回读，这里从不读回，显示不受影响。
 */
export function useFrameHold(params: {
  videoRef: React.RefObject<HTMLVideoElement | null>;
  seriesId: string | null;
  episodeKey: string;
  /** 播放侧真值（定义见 player-page）：出画 / 切换在途 / 流指纹。 */
  videoLive: boolean;
  switching: boolean;
  streamKey: string;
  playSrc: string | null;
  error: string | null;
  compatActive: boolean;
}) {
  const {
    videoRef,
    seriesId,
    episodeKey,
    videoLive,
    switching,
    streamKey,
    playSrc,
    error,
    compatActive,
  } = params;
  const [transition, setTransition] = useState<SwitchKind | null>(null);
  /**
   * 定格帧与抓帧时的流指纹：新流（≠抓帧来源）真正出画后据此判定帧已
   * 功成身退——抓帧来源自身还在播（取流窗口）时不算，否则帧会在切换
   * 途中被误清（清晰度切换全程 srcKey 不变，尤其踩这条）。
   */
  const [held, setHeld] = useState<{ canvas: HTMLCanvasElement; fromStream: string } | null>(null);
  const prevTargetRef = useRef<{ seriesId: string | null; episodeKey: string }>({
    seriesId: null,
    episodeKey: '',
  });

  /** 把当前画面定格成一张 canvas（拿不到画面就清空，回落封面）。 */
  const capture = useCallback(() => {
    const video = videoRef.current;
    if (!video || video.videoWidth === 0 || video.videoHeight === 0) {
      setHeld(null);
      return;
    }
    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    canvas.getContext('2d')?.drawImage(video, 0, 0);
    setHeld({ canvas, fromStream: streamKey });
  }, [videoRef, streamKey]);

  useLayoutEffect(() => {
    const prev = prevTargetRef.current;
    if (prev.episodeKey === episodeKey) return;
    prevTargetRef.current = { seriesId, episodeKey };
    if (!seriesId) return;
    const kind: SwitchKind = prev.seriesId === seriesId ? 'episode' : 'series';
    setTransition(kind);
    // 同剧切集才定格末帧；切剧走封面接管，定格帧绝不许跨剧泄漏
    if (kind === 'episode') capture();
    else setHeld(null);
  }, [seriesId, episodeKey, capture]);

  // 新流出画后定格帧淡出并清掉：一张 ~1080p 帧的 canvas 有几 MB，不留整集
  useEffect(() => {
    if (!held || !videoLive || held.fromStream === streamKey) return;
    const timer = setTimeout(() => setHeld(null), HOLD_FADE_MS);
    return () => clearTimeout(timer);
  }, [held, videoLive, streamKey]);

  const { holdVisible, coverVisible } = overlayVisibility({
    transition,
    videoLive,
    switching,
    hasFrame: held !== null,
    hasPlaySrc: playSrc !== null,
    error: error !== null,
    compat: compatActive,
  });

  return { frame: held?.canvas ?? null, holdVisible, coverHidden: !coverVisible, capture };
}

/**
 * 封面与定格帧的显隐真值表（纯函数，用例见 use-frame-hold.test.ts）：
 * - 定格帧：有帧、无出画画面、仍在视频分支（有流地址、无错误、无兜底）；
 * - 封面：画面真空（既没出画也没定格帧顶住），或切剧接管（switching 且
 *   非同剧切集——新剧的封面要立刻顶上，否则就是「切了没反应」）。
 *   同剧切集全程不出封面：取流窗口旧画面继续播，交换后定格帧接棒。
 */
export function overlayVisibility(v: {
  transition: SwitchKind | null;
  videoLive: boolean;
  switching: boolean;
  hasFrame: boolean;
  hasPlaySrc: boolean;
  error: boolean;
  compat: boolean;
}): { holdVisible: boolean; coverVisible: boolean } {
  const holdVisible = v.hasFrame && !v.videoLive && v.hasPlaySrc && !v.error && !v.compat;
  const coverVisible =
    !holdVisible && (!v.videoLive || (v.switching && v.transition !== 'episode'));
  return { holdVisible, coverVisible };
}
