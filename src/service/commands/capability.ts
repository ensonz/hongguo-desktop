import { call } from '../tauri/invoke';
import { hevcInstallResultSchema, playbackCapabilitySchema } from '../schema';

/** 播放解码能力：探测与 HEVC 扩展按需安装。 */
export const capability = {
  /** 本机 WebView2 能否解 HEVC（探测缓存在后端进程，会话内不变） */
  playbackCapability: () =>
    call<boolean>('playback_capability', undefined, playbackCapabilitySchema),
  /** 按需下载并安装 HEVC 视频扩展，返回安装后的能力重探结果 */
  installHevcExtension: () =>
    call<boolean>('install_hevc_extension', undefined, hevcInstallResultSchema),
};
