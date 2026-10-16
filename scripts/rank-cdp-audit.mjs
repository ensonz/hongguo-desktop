// 一次性审计脚本（用完即删）：全榜单 CDP 实测，确保 UI 不丢数据。
//
// 前提：app 以 CDP 启动（见 docs/webview-cdp-testing.md）。对每个
// 内容tab × 子榜：UI 切换 → 滚动到底 → 提取 DOM 标题序列；再在页面里
// 直接 invoke rank_list 全量分页取参照序列，逐位比对。任何缺位/错序/
// 重复都打印 FAIL 明细。
import { setTimeout as sleep } from 'node:timers/promises';

const CDP_HTTP = 'http://127.0.0.1:9222';
const APP_ORIGIN = 'http://localhost:1420';

async function connect() {
  const targets = await (await fetch(`${CDP_HTTP}/json`)).json();
  const page = targets.find((t) => t.type === 'page');
  if (!page) throw new Error('没有 page target');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  let seq = 0;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id) {
      const p = pending.get(m.id);
      if (!p) return;
      pending.delete(m.id);
      m.error ? p.rej(new Error(m.error.message)) : p.res(m.result);
    }
  };
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = () => rej(new Error('WebSocket 连接失败'));
  });
  const send = (method, params = {}) =>
    new Promise((res, rej) => {
      const id = ++seq;
      pending.set(id, { res, rej });
      ws.send(JSON.stringify({ id, method, params }));
    });
  return { send, close: () => ws.close() };
}

async function evaluate(send, expression) {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (r.exceptionDetails) {
    throw new Error(r.exceptionDetails.exception?.description ?? '页面里抛异常了');
  }
  return r.result?.value;
}

/** 稳定点击：点击挪出本上下文（重渲染会销毁执行环境），pointerdown+click 双派发。 */
const clickJs = (elJs) => `
  (() => {
    const el = ${elJs};
    if (!el) return 'no-el';
    setTimeout(() => {
      el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
      el.click();
    }, 0);
    return 'clicked';
  })()`;

const findTabJs = (name) =>
  `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(name)})`;
const findSubJs = (name) =>
  `[...document.querySelectorAll('[data-state] button, button[data-state]')].find(b => b.textContent.trim() === ${JSON.stringify(name)})
   ?? [...document.querySelectorAll('button')].filter(b => b.textContent.trim() === ${JSON.stringify(name)}).pop()`;

const domTitlesJs = `
  (() => {
    const first = document.querySelector('article[role=button]');
    const scroller = first?.closest('div[class*="overflow-y-auto"]');
    if (!scroller) return JSON.stringify({ titles: [], scrollerFound: false });
    const rows = [...scroller.querySelectorAll('article[role=button]')];
    return JSON.stringify({
      rows: rows.map((r) => ({
        t: r.getAttribute('aria-label'),
        // leading 名次角标是 article 的直接子 span（预约榜无名次则没有）
        rank: (r.querySelector(':scope > span')?.textContent ?? '').trim(),
      })),
      scrollerFound: true,
    });
  })()`;

async function scrollToListEnd(send) {
  // 滚到底：反复把滚动容器压底，条数连续 3 轮不涨即视为到底
  let prev = -1;
  let stable = 0;
  for (let round = 0; round < 40; round++) {
    await evaluate(
      send,
      `(() => {
        const first = document.querySelector('article[role=button]');
        const sc = first?.closest('div[class*="overflow-y-auto"]');
        if (sc) sc.scrollTop = sc.scrollHeight;
        return sc ? sc.scrollTop : 0;
      })()`,
    );
    await sleep(750);
    const { rows } = JSON.parse(await evaluate(send, domTitlesJs));
    if (rows.length === prev) {
      stable++;
      if (stable >= 3) return rows;
    } else {
      stable = 0;
      prev = rows.length;
    }
  }
  return JSON.parse(await evaluate(send, domTitlesJs)).rows;
}

/** 页面里直接 invoke 全量分页取参照序列。 */
const refFetchJs = (selected, sub) => `
  (async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    let offset = 0, session = '', rv = '', hasMore = true, guard = 0;
    const ids = [], titles = [];
    while (hasMore && guard++ < 40) {
      const r = await invoke('rank_list', {
        selected: ${JSON.stringify(selected)},
        sub: ${JSON.stringify(sub)},
        offset: offset || undefined,
        sessionId: session || undefined,
        rankVersion: rv || undefined,
      });
      ids.push(...r.items.map((i) => i.seriesId));
      titles.push(...r.items.map((i) => i.title));
      hasMore = r.hasMore;
      offset = r.nextOffset;
      session = r.sessionId;
      rv = r.rankVersion;
    }
    return JSON.stringify({ ids, titles, windows: guard - 1 });
  })()`;

function diffSeq(dom, ref) {
  const problems = [];
  if (dom.length !== ref.length) {
    problems.push(`条数 DOM=${dom.length} API=${ref.length}`);
  }
  const n = Math.max(dom.length, ref.length);
  for (let i = 0; i < n; i++) {
    if (dom[i] !== ref[i]) {
      problems.push(`第${i + 1}位不一致 DOM="${dom[i] ?? '<缺>'}" API="${ref[i] ?? '<缺>'}"`);
      if (problems.length >= 5) break;
    }
  }
  return problems;
}

const c = await connect();

// 落到排行榜页
await evaluate(c.send, `location.href = ${JSON.stringify(APP_ORIGIN + '/rank')}; 'nav'`);
await sleep(4000);
console.log('落地:', await evaluate(c.send, 'location.pathname'));

// 枚举全部 内容tab × 子榜（schema 由首页响应下发；演员 tab 前端不提供）
const tabs = JSON.parse(
  await evaluate(
    c.send,
    `window.__TAURI_INTERNALS__.invoke('rank_list',{selected:'all',sub:'ranklist_hot_sc'}).then(r=>JSON.stringify(r.tabs))`,
  ),
);
const contexts = tabs.flatMap((t) => t.subs.map((s) => ({ tab: t, sub: s })));
console.log(`共 ${contexts.length} 个榜单上下文\n`);

// 推荐榜族 = 个性化轮换榜：两次独立会话间池子会轮换，严格逐位比对
// 不适用（首屏互换/条数浮动是轮换噪声）。它们的正确口径：UI 无重复、
// 名次 1..N 连续、条目落在参照并集内。其余子榜是固定榜，严格比对。
const ROTATING = new Set([
  'ranklist_hot_sc',
  'human_hot_sc',
  'comic_series_hot_rank',
  'ai_playlet_hot_sc',
  'series_album_hot_sc',
]);

let failed = 0;
for (const { tab, sub } of contexts) {
  const label = `${tab.name}/${sub.name}`;
  try {
    // 切内容 tab（幂等：已激活再点一次无害）
    await evaluate(c.send, clickJs(findTabJs(tab.name)));
    await sleep(1500);
    // 切子榜
    await evaluate(c.send, clickJs(findSubJs(sub.name)));
    await sleep(2500); // 等首窗渲染

    const domRows = await scrollToListEnd(c.send);
    const domTitles = domRows.map((r) => r.t);
    const ref = JSON.parse(await evaluate(c.send, refFetchJs(tab.id, sub.id)));

    if (!ROTATING.has(sub.id)) {
      const problems = diffSeq(domTitles, ref.titles);
      if (problems.length === 0) {
        console.log(
          `PASS ${label}: UI ${domTitles.length} 条 = API ${ref.titles.length} 条（${ref.windows} 窗）`,
        );
      } else {
        failed++;
        console.log(`FAIL ${label}:`);
        for (const p of problems) console.log(`   - ${p}`);
      }
      continue;
    }

    // 轮换榜口径：查重 + 名次连续 + 参照并集覆盖（两遍参照降轮换噪声）
    const ref2 = JSON.parse(await evaluate(c.send, refFetchJs(tab.id, sub.id)));
    const universe = new Set([...ref.titles, ...ref2.titles]);
    const dupTitles = domTitles.filter((t, i) => domTitles.indexOf(t) !== i);
    const rankBreaks = domRows
      .map((r, i) => ({ r, i }))
      .filter(({ r, i }) => r.rank !== '' && Number(r.rank) !== i + 1)
      .slice(0, 3)
      .map(({ r, i }) => `第${i + 1}位名次显示 ${r.rank}`);
    const outside = domTitles.filter((t) => !universe.has(t));
    if (dupTitles.length === 0 && rankBreaks.length === 0) {
      console.log(
        `PASS ${label}: UI ${domTitles.length} 条（无重复/名次连续），参照并集 ${universe.size} 条，` +
          `轮换噪声 DOM 条数与单次参照差 ${Math.abs(domTitles.length - ref.titles.length)}`,
      );
      if (outside.length > 0) {
        console.log(
          `   WARN ${outside.length} 条不在参照并集（池子轮换新面孔）: ${outside.slice(0, 3).join(' / ')}`,
        );
      }
    } else {
      failed++;
      console.log(`FAIL ${label}:`);
      if (dupTitles.length > 0)
        console.log(
          `   - 重复 ${dupTitles.length} 条: ${[...new Set(dupTitles)].slice(0, 3).join(' / ')}`,
        );
      for (const b of rankBreaks) console.log(`   - ${b}`);
    }
  } catch (e) {
    failed++;
    console.log(`ERROR ${label}: ${e.message}`);
  }
}
console.log(failed === 0 ? '\n全部榜单 PASS' : `\n${failed} 个榜单 FAIL`);
c.close();
process.exit(failed === 0 ? 0 : 1);
