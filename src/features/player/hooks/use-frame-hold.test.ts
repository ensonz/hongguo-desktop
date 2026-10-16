import { describe, expect, it } from 'vitest';

import { overlayVisibility } from './use-frame-hold';

/**
 * 切集观感的回归锁：同剧切集全程不出封面（取流窗口旧画面继续播、交换后
 * 定格帧接棒），封面只属于切剧接管与画面真空；抓不到帧时回落封面不黑屏。
 */
describe('overlayVisibility', () => {
  const base = {
    transition: 'episode' as const,
    videoLive: true,
    switching: false,
    hasFrame: true,
    hasPlaySrc: true,
    error: false,
    compat: false,
  };

  it('同剧切集的取流窗口：旧画面还在播，封面与定格帧都不出', () => {
    const v = overlayVisibility({ ...base, switching: true });
    expect(v.holdVisible).toBe(false);
    expect(v.coverVisible).toBe(false);
  });

  it('流已交换、新集首帧未出：定格帧接棒，封面不闪', () => {
    const v = overlayVisibility({ ...base, videoLive: false });
    expect(v.holdVisible).toBe(true);
    expect(v.coverVisible).toBe(false);
  });

  it('新集首帧出画：定格帧退场，封面保持隐藏', () => {
    const v = overlayVisibility({ ...base });
    expect(v.holdVisible).toBe(false);
    expect(v.coverVisible).toBe(false);
  });

  it('切剧接管：旧画面还在播也要立即盖新剧封面（「切了没反应」的防线）', () => {
    const v = overlayVisibility({ ...base, transition: 'series', switching: true });
    expect(v.coverVisible).toBe(true);
    expect(v.holdVisible).toBe(false);
  });

  it('初次挂载（transition 未定、不可能有帧）等同切剧：取流窗口出封面', () => {
    const v = overlayVisibility({
      ...base,
      transition: null,
      hasFrame: false,
      switching: true,
      videoLive: false,
    });
    expect(v.holdVisible).toBe(false);
    expect(v.coverVisible).toBe(true);
  });

  it('抓不到帧（老 WebView/黑屏切换）回落封面，不裸黑屏', () => {
    const v = overlayVisibility({ ...base, hasFrame: false, videoLive: false, switching: true });
    expect(v.holdVisible).toBe(false);
    expect(v.coverVisible).toBe(true);
  });

  it('清晰度切换同款：取流窗口不出封面，交换后定格帧接棒', () => {
    // 清晰度切换全程 switching=false（srcKey 不换集）
    const fetchWindow = overlayVisibility({ ...base, switching: false });
    expect(fetchWindow.coverVisible).toBe(false);
    const swapGap = overlayVisibility({ ...base, videoLive: false });
    expect(swapGap.holdVisible).toBe(true);
    expect(swapGap.coverVisible).toBe(false);
  });

  it('错误/兜底转码在途时定格帧让位（错误面板与封面照旧接管）', () => {
    for (const patch of [{ error: true }, { compat: true }, { hasPlaySrc: false }] as const) {
      const v = overlayVisibility({ ...base, videoLive: false, ...patch });
      expect(v.holdVisible).toBe(false);
      expect(v.coverVisible).toBe(true);
    }
  });
});
