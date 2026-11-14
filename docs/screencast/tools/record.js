// Records the Cedar intro screencast: drives the chat UI in headless Chromium, captures 1920x1080
// frames with their timestamps, and writes frames/, frames.txt (for ffmpeg's concat demuxer) and
// timeline.json (when each scene of narration.json starts and ends).
//
//   node record.js <out-dir> [lang] [audio-dir]
//
// The stack must be running fresh, with the scripted model in the same language (see start.sh).
// lang is en (default), de, fr or es: the chat is switched to it, and what is typed and shown
// comes from content.py. With audio-dir (the clips voice.sh made), each scene lasts as long as
// its clip; without, as long as its narration is estimated to take. Nothing here finds an element
// by its English label, so the same steps work in every language.
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const OUT = path.resolve(process.argv[2] || 'take');
const URL = process.env.CHAT_URL || 'http://127.0.0.1:3000';
const W = 1440, H = 810, SCALE = 4 / 3;         // 1920x1080 frames
const WPS = 2.45, PAD = 1.2;                   // narration pace (words a second) and air after it
const CPS = 13.5;                              // the same pace in characters, for the translations
const LANG = process.argv[3] || 'en';
const AUDIO = process.argv[4] ? path.resolve(process.argv[4]) : null;
const { execFileSync } = require('child_process');
const C = JSON.parse(execFileSync('python3', [path.join(__dirname, 'content.py'), LANG], { encoding: 'utf8' }));
const NARRATION = JSON.parse(fs.readFileSync(path.join(__dirname, 'narration.json'), 'utf8'));
if (LANG !== 'en') {
  const texts = JSON.parse(fs.readFileSync(path.join(__dirname, `narration.${LANG}.json`), 'utf8'));
  for (const s of NARRATION) s.text = texts[s.id];
}
// How long a scene must last: its clip if there is one, else the narration at the usual pace.
function sceneSeconds(index, n) {
  if (AUDIO) {
    const clip = ['mp3', 'wav'].map(x => path.join(AUDIO, `${String(index + 1).padStart(2, '0')}-${n.id}.${x}`)).find(f => fs.existsSync(f));
    if (clip) return parseFloat(execFileSync('ffprobe', ['-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', clip], { encoding: 'utf8' })) + 0.4 + 0.8;
  }
  return (LANG === 'en' ? n.text.split(/\s+/).length / WPS : n.text.length / CPS) + PAD;
}

const sleep = ms => new Promise(r => setTimeout(r, ms));
const MARK = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect x="6" y="6" width="36" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="16" width="12" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="26" width="12" height="6" rx="2" fill="#3a5f4b"/><rect x="6" y="36" width="36" height="6" rx="2" fill="#3a5f4b"/><rect x="33" y="21" width="9" height="6" rx="2" fill="#b5553a"/></svg>';

function card(body) {
  return `<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/fonts/fonts.css"><style>
    html,body{height:100%;margin:0;background:#f7f5f0;color:#1c1b19;font-family:Figtree,sans-serif;-webkit-font-smoothing:antialiased}
    body{display:flex;flex-direction:column;align-items:center;justify-content:center;text-align:center}
    .lock{display:flex;align-items:center;gap:18px;font-weight:600;font-size:54px;letter-spacing:-.02em;margin-bottom:44px}
    .lock svg{width:76px;height:76px}
    h1{font-family:Lora,serif;font-weight:500;font-size:68px;line-height:1.12;letter-spacing:-.015em;margin:0 0 28px}
    p{font-size:25px;line-height:1.5;color:#6b685f;margin:0;max-width:760px}
    .row{display:flex;gap:12px;margin-top:40px;flex-wrap:wrap;justify-content:center}
    .chip{font-size:19px;padding:9px 18px;border:1px solid #e3dfd4;border-radius:999px;background:#fff;color:#3a3833}
    .url{margin-top:44px;font-family:"JetBrains Mono",monospace;font-size:21px;color:#1f7a5c}
    body>*{opacity:0;transform:translateY(10px);animation:in .7s ease forwards}
    body>*:nth-child(2){animation-delay:.25s} body>*:nth-child(3){animation-delay:.5s}
    body>*:nth-child(4){animation-delay:.75s} body>*:nth-child(5){animation-delay:1s}
    @keyframes in{to{opacity:1;transform:none}}
  </style>${body}`;
}
const CARDS = {
  title: card(`<div class="lock">${MARK}<span>Cedar</span></div><h1>${C.card_title}</h1>
    <p>${C.card_sub}</p>`),
  outro: card(`<div class="lock">${MARK}<span>Cedar</span></div><h1>${C.card_title}</h1>
    <div class="row"><span class="chip">Mac</span><span class="chip">Windows</span><span class="chip">Linux</span><span class="chip">iPhone</span><span class="chip">Android</span></div>
    <div class="url">github.com/hanneshapke/cedar</div><p style="font-size:19px;margin-top:14px">${C.card_free}</p>`),
};

// A cursor the capture can see: headless Chromium draws none. It sits in the top layer (a
// popover) and is re-shown whenever a dialog opens, so modal dialogs don't cover it.
const CURSOR = () => {
  if (window.__cur || location.pathname.startsWith('/__card')) return;
  const make = () => {
    const c = document.createElement('div');
    c.id = '__cur';
    c.setAttribute('popover', 'manual');
    c.style.cssText = 'position:fixed;inset:auto;left:0;top:0;margin:0;padding:0;border:0;background:none;overflow:visible;width:0;height:0;pointer-events:none;z-index:2147483647;transition:transform .12s';
    c.innerHTML = '<svg width="26" height="26" viewBox="0 0 24 24" style="position:absolute;left:-3px;top:-2px;filter:drop-shadow(0 1px 2px rgba(0,0,0,.35))"><path d="M4 2l15.5 9.2-6.6 1.5 4 7.3-2.8 1.5-4-7.3L5 19z" fill="#1c1b19" stroke="#fff" stroke-width="1.4" stroke-linejoin="round"/></svg><i style="position:absolute;left:-18px;top:-18px;width:36px;height:36px;border-radius:50%;background:rgba(31,122,92,.28);transform:scale(0);transition:transform .25s,opacity .35s;opacity:0"></i>';
    document.documentElement.append(c);
    const place = () => { c.style.left = (window.__cx ?? -50) + 'px'; c.style.top = (window.__cy ?? -50) + 'px'; };
    const raise = () => { try { c.hidePopover(); } catch {} try { c.showPopover(); } catch {} place(); };
    raise();
    addEventListener('mousemove', e => { window.__cx = e.clientX; window.__cy = e.clientY; place(); }, true);
    addEventListener('mousedown', () => { const i = c.querySelector('i'); i.style.transition = 'none'; i.style.transform = 'scale(.3)'; i.style.opacity = '1'; requestAnimationFrame(() => { i.style.transition = ''; i.style.transform = 'scale(1)'; i.style.opacity = '0'; }); }, true);
    let open = 0;
    setInterval(() => { const n = document.querySelectorAll('dialog[open], [popover]:popover-open:not(#__cur)').length; if (n !== open) { open = n; raise(); } }, 60);
    window.__cur = c;
  };
  if (document.documentElement && document.body) make(); else addEventListener('DOMContentLoaded', make);
};

(async () => {
  fs.rmSync(OUT, { recursive: true, force: true });
  fs.mkdirSync(path.join(OUT, 'frames'), { recursive: true });
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: W, height: H }, locale: C.locale, timezoneId: 'Europe/Berlin' });
  const page = await ctx.newPage();
  page.setDefaultTimeout(15000);
  page.on('console', m => m.type() === 'error' && console.log('  console error:', m.text().slice(0, 160)));
  await page.addInitScript(CURSOR);
  await page.route('**/__card/*', r => r.fulfill({ contentType: 'text/html', body: CARDS[r.request().url().split('/').pop()] }));
  const cdp = await ctx.newCDPSession(page);

  // --- capture -----------------------------------------------------------------------------
  const frames = [];
  let recording = true, t0 = null, hold = false;
  const now = () => (t0 === null ? 0 : (Date.now() - t0) / 1000);
  const captureLoop = async () => {
    while (recording) {
      try {
        const at = Date.now();
        if (hold) { await sleep(20); continue; }
        // A capture that is pending when the page navigates never answers, hence the race.
        const shot = cdp.send('Page.captureScreenshot', { format: 'jpeg', quality: 95, optimizeForSpeed: true, clip: { x: 0, y: 0, width: W, height: H, scale: SCALE } });
        shot.catch(() => {});
        const res = await Promise.race([shot, sleep(1500).then(() => null)]);
        if (!res) continue;
        const { data } = res;
        if (t0 === null) t0 = at;
        const name = `frames/${String(frames.length).padStart(5, '0')}.jpg`;
        fs.writeFileSync(path.join(OUT, name), Buffer.from(data, 'base64'));
        frames.push({ t: (at - t0) / 1000, name });
      } catch (e) { await sleep(30); }
    }
  };
  let capture;

  // --- helpers -----------------------------------------------------------------------------
  let mx = W / 2, my = H / 2;
  const loc = t => (typeof t === 'string' ? page.locator(t).first() : t);
  async function glide(x, y, ms) {
    const d = Math.hypot(x - mx, y - my);
    ms = ms ?? Math.min(900, 260 + d * 0.9);
    const n = Math.max(6, Math.round(ms / 22)), sx = mx, sy = my;
    for (let i = 1; i <= n; i++) {
      const k = i / n, e = k < .5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2;
      await page.mouse.move(sx + (x - sx) * e, sy + (y - sy) * e);
      await sleep(16);
    }
    mx = x; my = y;
  }
  async function point(target, { dx = 0, dy = 0, at } = {}) {
    const l = loc(target);
    await l.waitFor({ state: 'visible' });
    await l.scrollIntoViewIfNeeded().catch(() => {});
    await sleep(120);
    const b = await l.boundingBox();
    const x = at ? b.x + b.width * at[0] : b.x + b.width / 2 + dx, y = at ? b.y + b.height * at[1] : b.y + b.height / 2 + dy;
    await glide(x, y);
  }
  async function click(target, opts) {
    await point(target, opts);
    await sleep(180);
    await page.mouse.down(); await sleep(60); await page.mouse.up();
    await sleep(350);
  }
  async function type(text, delay = 26) { await page.keyboard.type(text, { delay }); }
  async function wheel(target, dy, ms = 900) {
    await point(target);
    const n = Math.round(ms / 30);
    for (let i = 0; i < n; i++) { await page.mouse.wheel(0, dy / n); await sleep(30); }
  }
  const visible = async sel => page.locator(sel).first().isVisible().catch(() => false);

  // --- scenes ------------------------------------------------------------------------------
  const timeline = [];
  let cur = null;
  async function scene(id) {
    if (cur) {
      const need = cur.min - (now() - cur.start);
      if (need > 0) await sleep(need * 1000);
      cur.end = now();
      console.log(`${cur.id.padEnd(10)} ${cur.start.toFixed(1).padStart(6)} → ${cur.end.toFixed(1).padStart(6)}  (narration needs ${cur.min.toFixed(1)}s, action took ${(cur.action).toFixed(1)}s)`);
    }
    if (!id) return;
    const index = NARRATION.findIndex(s => s.id === id);
    cur = { id, start: now(), min: sceneSeconds(index, NARRATION[index]) };
    timeline.push(cur);
  }
  const acted = () => { cur.action = now() - cur.start; };
  const until = async frac => { const need = cur.min * frac - (now() - cur.start); if (need > 0) await sleep(need * 1000); };

  // 1. Title card
  console.log('opening title card');
  await page.goto(`${URL}/__card/title`);
  capture = captureLoop();
  await sleep(400);
  await scene('title'); acted();

  // 2. First start: the language, then the empty welcome page
  await scene('first-run');
  hold = true; await sleep(150);
  await page.goto(URL + '/');
  hold = false;
  await page.locator('.lang-dialog-go').waitFor();
  await page.evaluate(() => { for (const e of [document.querySelector('.lang-dialog'), ...document.querySelectorAll('.lang-dialog *')]) e.scrollTop = 0; });
  await page.mouse.move(mx, my);
  await sleep(1800);
  const option = tag => `.lang-dialog button.ui-option[data-locale="${tag}"]`;
  await point(option(C.hover_locale)); await sleep(500);
  if (C.locale === 'en-US') { await point(option(C.locale)); await sleep(400); } else await click(option(C.locale));
  await wheel(option('fr-FR'), 400, 700);
  await click('.lang-dialog-go');
  await page.locator('.lang-dialog').waitFor({ state: 'detached' }).catch(() => {});
  await page.locator('#input').waitFor();
  await sleep(700);
  await page.mouse.move(mx, my);
  await point('#input'); acted();

  // 3. Settings → Models → Add a model
  await scene('models');
  const nav = i => page.locator('.settings-nav-item').nth(i);   // Interface, Language, Tools, Models, Memory, ...
  await sleep(1500);
  await click('#more-btn');
  await click('#settings-btn');
  await sleep(700);
  await click(nav(3));
  await until(0.14);
  await click('dialog[open] button.primary:not(#settings-save):visible');
  await sleep(600);
  const choice = i => page.locator('button.cedar-set-choice').nth(i);   // llama.cpp, hosted, a server you run
  await point(choice(0), { at: [0.3, 0.5] });
  await until(0.36);
  await point(choice(1), { at: [0.3, 0.5] });
  await until(0.46);
  await click(choice(2), { at: [0.3, 0.5] });
  await until(0.56);
  await click('.cedar-set-presets button:text-is("vLLM")');
  await sleep(900);
  await click('.cedar-set-server-models button');
  await sleep(1200);
  const pick = page.locator('.cedar-set-server-models select');
  await point(pick); await sleep(300);
  await pick.selectOption({ index: 1 });
  await sleep(700);
  await click(page.locator('.cedar-set-endpoint input[type=text]').nth(1));   // address, label, id
  await type('DeepSeek V4 Flash', 45);
  await sleep(400);
  await page.locator('.cedar-set-reasoning').scrollIntoViewIfNeeded();
  await point('.cedar-set-reasoning span');
  await until(0.84);
  await click('.cedar-set-endpoint label.cedar-set-check input[type=checkbox]');
  await sleep(500);
  await click('.cedar-set-endpoint button.primary');
  await page.locator('.cedar-set-endpoint').waitFor({ state: 'detached' });
  await sleep(500);
  await page.evaluate(() => { for (const e of document.querySelectorAll('dialog[open] *')) if (e.scrollTop > 0) e.scrollTo({ top: 0, behavior: 'smooth' }); });
  await point(page.locator('dialog[open]').getByText('DeepSeek V4 Flash', { exact: true }).locator('visible=true').first(), { dx: 120 });
  await sleep(1600); acted();
  await until(1);
  await click('#settings-close');

  // 4. The task: a file and a request
  await scene('task');
  await sleep(600);
  const chooser = page.waitForEvent('filechooser');
  await click('.cedar-attach-btn');
  await (await chooser).setFiles(path.join(__dirname, C.csv));
  await sleep(900);
  await click('#input');
  await type(C.prompt, 30);
  await sleep(700); acted();
  await until(1);
  await click('#send');

  // 5. The plan
  await scene('plan');
  await page.locator('.cedar-plan-start').waitFor();
  await sleep(1400);
  const steps = page.locator('.cedar-plan-remove');
  await point(steps.nth(1), { dx: -200 }); await sleep(900);
  await point(steps.nth(3)); await sleep(900);
  await point('.cedar-plan-add'); await sleep(900);
  await point('.cedar-plan-start'); acted();
  await until(1);
  await click('.cedar-plan-start');

  // 6. The work, as it happens
  await scene('work');
  await glide(1010, 330);
  await page.locator('article.card.trace').first().waitFor({ timeout: 60000 });
  await sleep(1500);
  await page.locator('article.card.doc').first().waitFor({ timeout: 60000 });
  await page.locator('.status.live').waitFor({ state: 'detached', timeout: 60000 }).catch(() => {});
  await page.locator('#agent-btn').waitFor({ timeout: 60000 }).catch(() => {});
  await sleep(1200);
  // open the Python step: what ran, and what it printed
  await click(page.locator('.work-sum').first());
  await sleep(500);
  await click(page.locator('article.card.trace .card-head').first());
  await sleep(1200);
  acted();
  await until(1);

  // 7. The result: the chart, the document, the answer
  await scene('result');
  await click(page.locator('article.card.trace .card-head').first());
  const stageTop = async l => (await l.boundingBox()).y - (await page.locator('#stage').boundingBox()).y;
  const widget = page.locator('article.card.widget').first();
  await wheel('#stage', (await stageTop(widget)) - 40, 700);
  const svg = widget.locator('svg:has(rect)').last();
  for (const fx of [0.16, 0.5, 0.68, 0.77]) { await point(svg, { at: [fx, 0.45] }); await sleep(1100); }
  await until(0.38);
  const doc = page.locator('article.card.doc').first();
  await wheel('#stage', (await stageTop(doc)) - 30, 900);
  await sleep(1200);
  await wheel('#stage', 760, 2600);
  await until(0.8);
  await glide(230, 470);
  for (let i = 0; i < 40; i++) { await page.mouse.wheel(0, 45); await sleep(30); }
  acted();
  await until(1);

  // 8. A follow-up that needs an OK
  await scene('approval');
  await click('#input');
  await type(C.prompt_delete, 30);
  await sleep(400);
  await click('#send');
  await page.locator('.cedar-approval-btn.primary').waitFor({ timeout: 60000 });
  await sleep(900);
  await point('.cedar-approval-btn.primary');
  await until(0.72);
  await click('.cedar-approval-btn.primary');
  await page.getByText(C.done_delete.split('. ')[1].slice(0, 30)).first().waitFor({ timeout: 60000 });
  await sleep(800); acted();
  await until(1);

  // 9. Memory
  await scene('memory');
  await click('#input');
  await type(C.prompt_remember, 28);
  await sleep(300);
  await click('#send');
  await page.getByText(C.noted.split('.')[0] + '.').first().waitFor({ timeout: 60000 });
  await sleep(1500);
  await click('#more-btn');
  await click('#settings-btn');
  await sleep(500);
  await click(nav(4));
  await sleep(1200);
  const kept = page.locator('dialog[open] .cedar-set-integration-side:visible').first().locator('button');   // Pin, Archive, Remove
  for (let i = 0; i < 3; i++) { await point(kept.nth(i)); await sleep(650); }
  acted();
  await until(1);

  // 10. Tools: a switch each
  await scene('tools');
  await click(nav(2));
  await sleep(900);
  const rows = page.locator('dialog[open] .cedar-set-row:visible');   // web search, web pages, weather, maps, recipes, transit, flights, stocks, ...
  for (const i of [1, 2, 3, 5]) { await point(rows.nth(i).locator('.cedar-set-row-name')); await sleep(450); }
  await point(rows.nth(7).locator('.cedar-set-row-name')); await sleep(300);
  await click(rows.nth(7).locator('.cedar-set-switch'));
  await sleep(1400); acted();
  await until(1);
  await click('#settings-close');

  // 11. An agent from the conversation
  await scene('agent');
  await sleep(500);
  await click('#agent-btn');
  const name = page.locator('.cedar-ag-form input[type=text]').first();
  await name.waitFor();
  await page.waitForFunction(() => { const t = document.querySelector('.cedar-ag-form textarea'); return t && t.value.length > 40; }, null, { timeout: 60000 });
  await sleep(900);
  await click(name);
  await page.keyboard.press('Control+A');
  await type(C.agent_name, 40);
  await point('.cedar-ag-form textarea', { at: [0.5, 0.3] });
  await until(0.42);
  const starts = v => `.cedar-ag-pick:has(input[value="${v}"]) b`;
  for (const v of ['condition', 'email', 'drive', 'schedule']) { await point(starts(v)); await sleep(700); }
  const often = page.locator('.cedar-ag-set select').first();
  await often.scrollIntoViewIfNeeded();
  await point(often); await sleep(300);
  await often.selectOption('monthly');
  await sleep(1200);
  const create = page.locator('.cedar-ag-form button.primary').last();
  await create.scrollIntoViewIfNeeded();
  await until(0.85);
  await click(create);
  await sleep(2500); acted();
  await until(1);

  // 12. Closing card
  await scene('outro');
  hold = true; await sleep(150);
  await page.goto(`${URL}/__card/outro`);
  hold = false; acted();
  await sleep(1500);

  await scene(null);
  recording = false; await capture;
  const end = now();
  await browser.close();

  // frames.txt for ffmpeg, with each frame shown until the next one
  let list = '';
  frames.forEach((f, i) => { list += `file '${f.name}'\nduration ${((frames[i + 1]?.t ?? end) - f.t).toFixed(3)}\n`; });
  list += `file '${frames[frames.length - 1].name}'\n`;
  fs.writeFileSync(path.join(OUT, 'frames.txt'), list);
  fs.writeFileSync(path.join(OUT, 'timeline.json'), JSON.stringify({ duration: end, scenes: timeline }, null, 2));
  console.log(`${frames.length} frames, ${end.toFixed(1)}s, ${(frames.length / end).toFixed(1)} fps`);
})().catch(e => { console.error(e); process.exit(1); });
