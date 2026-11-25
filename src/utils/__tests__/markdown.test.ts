import { describe, it, expect, vi, beforeEach } from 'vitest';

const convertFileSrcMock = vi.fn((filePath: string, protocol?: string) => {
  const proto = protocol || 'asset';
  return `http://${proto}.localhost/${encodeURIComponent(filePath)}`;
});

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (...args: [string, string?]) => convertFileSrcMock(...args),
}));

import { resolveMarkdownImageSrc, renderMarkdown } from '../markdown';

describe('markdown 图片地址解析与渲染', () => {
  beforeEach(() => {
    convertFileSrcMock.mockClear();
  });

  it('attachment:// 协议走 jarvis-attachment 自定义协议', () => {
    const out = resolveMarkdownImageSrc('attachment://45dc870d_148726fc.jpg');
    expect(convertFileSrcMock).toHaveBeenCalledWith(
      '45dc870d_148726fc.jpg',
      'jarvis-attachment'
    );
    expect(out).toBe('http://jarvis-attachment.localhost/45dc870d_148726fc.jpg');
  });

  it('attachment:// 大小写不敏感并忽略首尾空白', () => {
    resolveMarkdownImageSrc('  ATTACHMENT://abc_123.png  ');
    expect(convertFileSrcMock).toHaveBeenCalledWith('abc_123.png', 'jarvis-attachment');
  });

  it('本地绝对图片路径与 file:// URI 走默认 asset 协议转换', () => {
    resolveMarkdownImageSrc('E:/demo/shot.png');
    expect(convertFileSrcMock).toHaveBeenCalledWith('E:/demo/shot.png');

    convertFileSrcMock.mockClear();
    resolveMarkdownImageSrc('file:///C:/Users/demo/pic%201.jpg');
    expect(convertFileSrcMock).toHaveBeenCalledWith('C:/Users/demo/pic 1.jpg');
  });

  it('远程链接、data URI 与非图片本地路径不走 convertFileSrc', () => {
    expect(resolveMarkdownImageSrc('https://example.com/a.png')).toBe(
      'https://example.com/a.png'
    );
    expect(resolveMarkdownImageSrc('data:image/png;base64,AAA')).toBe(
      'data:image/png;base64,AAA'
    );
    expect(resolveMarkdownImageSrc('E:/demo/secret.txt')).toBe('E:/demo/secret.txt');
    expect(convertFileSrcMock).not.toHaveBeenCalled();
  });

  it('renderMarkdown 将 attachment:// 图片渲染为 inline-block 的 span 包裹 img', () => {
    const html = renderMarkdown('![截图](attachment://shot_1.png)');
    expect(html).toContain('<span class="markdown-image-wrap">');
    expect(html).toContain(
      'src="http://jarvis-attachment.localhost/shot_1.png"'
    );
    expect(html).toContain('alt="截图"');
  });
});
