import { describe, expect, it } from 'vitest';
import { bossKeyConflicts, bossKeyCoreKey } from '@/utils/boss-key';

describe('bossKeyCoreKey', () => {
  it('取末段裸键并小写化', () => {
    expect(bossKeyCoreKey('CommandOrControl+Alt+H')).toBe('h');
    expect(bossKeyCoreKey('Alt+Z')).toBe('z');
  });

  it('空串与残串返回空（不参与冲突比对）', () => {
    expect(bossKeyCoreKey('')).toBe('');
    expect(bossKeyCoreKey('   ')).toBe('');
  });
});

describe('bossKeyConflicts', () => {
  const actions = [
    { action: 'playPause', label: '空格', key: ' ' },
    { action: 'seekBack', label: '←', key: 'ArrowLeft' },
    { action: 'seekForward', label: '→', key: 'ArrowRight' },
    { action: 'stepPrev', label: '↑', key: 'ArrowUp' },
    { action: 'stepNext', label: 'h', key: 'h' },
  ];

  it('裸键与播放器键相同（忽略大小写）即冲突', () => {
    expect(bossKeyConflicts('CommandOrControl+Alt+H', actions)).toHaveLength(1);
    expect(bossKeyConflicts('Alt+h', actions)).toHaveLength(1);
    expect(bossKeyConflicts('CommandOrControl+Alt+B', actions)).toHaveLength(0);
  });

  it('未启用（空串）不报冲突', () => {
    expect(bossKeyConflicts('', actions)).toHaveLength(0);
  });

  it('冲突条目携带动作标识与可读键名', () => {
    const conflicts = bossKeyConflicts('CommandOrControl+Alt+H', actions);
    expect(conflicts[0]).toEqual({ action: 'stepNext', label: 'h' });
  });
});
