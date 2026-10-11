import { call } from '../tauri/invoke';
import type { ProxyConfig, Settings } from '../schema';

// ---------------------------------------------------------------- 设置

export const settings = {
  get: () => call('get_settings'),
  save: (next: Settings) => call('save_settings', { settings: next }),
  testProxy: (draft?: ProxyConfig) => call('test_proxy', { draft: draft ?? null }),
};
