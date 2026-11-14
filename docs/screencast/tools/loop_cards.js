// Draws the still pictures loop.py lays over the short films: the reel's frame (the Cedar mark, a
// line of text per part of the loop) and the closing cards. Called by loop.py:
//
//   node loop_cards.js <out-dir> '<json: {tagline, steps: [..4], free, url}>'
//
// Writes reel-0.png .. reel-3.png (1080x1920, transparent where the film shows), reel-end.png
// and wide-end.png (1920x1080). The fonts are the chat's own, read from chat/static/fonts.
const { chromium } = require('playwright');
const path = require('path');
const [out, json] = process.argv.slice(2);
const T = JSON.parse(json);
const FONTS = path.resolve(__dirname, '../../../chat/static/fonts');
const MARK = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect x="6" y="6" width="36" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="16" width="12" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="26" width="12" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="36" width="36" height="6" rx="2" fill="#3a5f4b"/><rect x="33" y="21" width="9" height="6" rx="2" fill="#b5553a"/></svg>';
const css = `
  @font-face{font-family:Figtree;font-weight:400 600;src:url("file://${FONTS}/figtree-latin.woff2") format("woff2")}
  @font-face{font-family:Lora;font-weight:400 600;src:url("file://${FONTS}/lora-latin.woff2") format("woff2")}
  @font-face{font-family:Mono;src:url("file://${FONTS}/jetbrains-mono-latin.woff2") format("woff2")}
  html,body{margin:0;height:100%;font-family:Figtree,sans-serif;color:#1c1b19;-webkit-font-smoothing:antialiased}
  .lock{display:flex;align-items:center;justify-content:center;gap:16px;font-weight:600;letter-spacing:-.02em}
  .lock svg{display:block}
  h1{font-family:Lora,serif;font-weight:500;letter-spacing:-.015em;margin:0;text-align:center;line-height:1.12}
  .url{font-family:Mono,monospace;color:#1f7a5c;text-align:center}
  .free{color:#6b685f;text-align:center}`;
// The reel: the film sits in a 1000x1140 box at (40, 450); everything else is drawn here.
const reel = (step) => `<style>${css} body{background:transparent}
  .lock{position:absolute;left:0;right:0;top:150px;font-size:64px}.lock svg{width:84px;height:84px}
  h1{position:absolute;left:60px;right:60px;top:278px;height:150px;display:flex;align-items:center;justify-content:center;font-size:54px}
  .url{position:absolute;left:0;right:0;top:1622px;font-size:30px}</style>
  <div class="lock">${MARK}<span>Cedar</span></div><h1>${step}</h1><div class="url">${T.url}</div>`;
const end = (w, h, k) => `<style>${css} body{background:#f7f5f0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:${36 * k}px}
  .lock{font-size:${60 * k}px}.lock svg{width:${84 * k}px;height:${84 * k}px}
  h1{font-size:${64 * k}px;max-width:${w - 120}px}.url{font-size:${26 * k}px}.free{font-size:${22 * k}px}</style>
  <div class="lock">${MARK}<span>Cedar</span></div><h1>${T.tagline}</h1><div class="url">${T.url}</div><div class="free">${T.free}</div>`;
(async () => {
  const browser = await chromium.launch();
  const shot = async (name, w, h, html, clear) => {
    const page = await browser.newPage({ viewport: { width: w, height: h } });
    await page.setContent('<!doctype html><meta charset="utf-8">' + html);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: path.join(out, name), omitBackground: clear });
    await page.close();
  };
  for (let i = 0; i < T.steps.length; i++) await shot(`reel-${i}.png`, 1080, 1920, reel(T.steps[i]), true);
  await shot('reel-end.png', 1080, 1920, end(1080, 1920, 1.25), false);
  await shot('wide-end.png', 1920, 1080, end(1920, 1080, 1.25), false);
  await browser.close();
})();
