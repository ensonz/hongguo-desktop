import { describe, expect, it } from 'vitest';

import { classifyMediaError } from './media-error';

describe('classifyMediaError', () => {
  it('解码类（3 DECODE / 4 SRC_NOT_SUPPORTED）判 decode——确定性失败重试无意义', () => {
    expect(classifyMediaError(3)).toBe('decode');
    expect(classifyMediaError(4)).toBe('decode');
  });

  it('网络类（2）判 network——重试换流有意义', () => {
    expect(classifyMediaError(2)).toBe('network');
  });

  it('aborted（1）、0 与缺失归 other——维持原「重试一次 → 报错」路径', () => {
    expect(classifyMediaError(1)).toBe('other');
    expect(classifyMediaError(0)).toBe('other');
    expect(classifyMediaError(undefined)).toBe('other');
    expect(classifyMediaError(null)).toBe('other');
  });
});
