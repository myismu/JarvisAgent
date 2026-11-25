import { describe, it, expect } from 'vitest';
import { extractUserImages, plainUserText } from '../userMessage';

describe('userMessage 历史图片提取', () => {
  it('从历史用户消息 HTML 中提取 data URI 图片，文本仍按纯文本展示', () => {
    const html =
      '<img src="data:image/jpeg;base64,AAAA" style="max-width: 200px;" alt="图片" />\n这是谁';
    expect(extractUserImages(html)).toEqual(['data:image/jpeg;base64,AAAA']);
    expect(plainUserText(html).trim()).toBe('这是谁');
  });

  it('多张图片按顺序提取；无图片或非 data URI 返回空数组', () => {
    const html =
      '<img src="data:image/png;base64,BBBB" /><img src="data:image/webp;base64,CCCC" />hi';
    expect(extractUserImages(html)).toEqual([
      'data:image/png;base64,BBBB',
      'data:image/webp;base64,CCCC',
    ]);
    expect(extractUserImages('纯文本')).toEqual([]);
    expect(extractUserImages('<img src="https://x/y.png" />')).toEqual([]);
    expect(extractUserImages(undefined)).toEqual([]);
  });
});
