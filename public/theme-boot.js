// 首帧前涂主题：主题目前由 main.tsx 在整个模块图执行完之后才涂上，
// WebView 默认白底会先亮出来（启动白屏的一部分）。本脚本在 <head> 里
// 同步执行（经典脚本阻塞解析，保证任何绘制发生前生效），读 zustand
// persist 的主题键预挂 dark 类并给 <html> 铺底色。
//
// CSP 是 default-src 'self'（不放开 unsafe-inline），所以必须是外部
// 文件而不是内联脚本。底色两个值与 styles/index.css 的 --background
// 语义令牌一一对应（暗 oklch(0.145 0 0)=#0a0a0a，亮 oklch(1 0 0)=#fff），
// 改主题令牌时同步这里。
(function () {
  var theme = 'auto';
  try {
    var raw = localStorage.getItem('hongguo-theme');
    if (raw) {
      var parsed = JSON.parse(raw);
      if (parsed && parsed.state && parsed.state.theme) theme = parsed.state.theme;
    }
  } catch (e) {
    // 读不到就按默认 auto 走系统偏好
  }
  var dark =
    theme === 'dark' ||
    (theme !== 'light' && window.matchMedia('(prefers-color-scheme: dark)').matches);
  if (dark) document.documentElement.classList.add('dark');
  document.documentElement.style.backgroundColor = dark ? '#0a0a0a' : '#ffffff';
})();
