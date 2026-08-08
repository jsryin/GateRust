export async function copyText(text: string): Promise<void> {
  const clipboard = navigator.clipboard;
  if (typeof clipboard?.writeText === 'function') {
    try {
      await clipboard.writeText(text);
      return;
    } catch {
      // 非安全上下文或权限受限时，回退到仍受主流浏览器支持的同步复制方式。
    }
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.readOnly = true;
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  textarea.style.pointerEvents = 'none';
  document.body.append(textarea);
  textarea.select();
  textarea.setSelectionRange(0, text.length);

  try {
    if (typeof document.execCommand !== 'function' || !document.execCommand('copy')) {
      throw new Error('当前浏览器不支持复制到剪贴板');
    }
  } finally {
    textarea.remove();
  }
}
