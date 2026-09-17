/* harden/clarify/adapt 修复验证 — 针对审计修复项的黑盒回归
 *
 * 覆盖：
 *   N1  模式仲裁：demo → 返回 → 加载真实文件，不再残留演示数据
 *   N2  筛选空集语义：None 真正清空 + Reset Filters 可恢复
 *   N3  小屏抽屉：<lg 面板滑入/遮罩关闭
 *   N4  i18n 持久化：选择语言后刷新保持
 *   N5  键盘可达：Tab 聚焦拖放区，Enter 触发文件选择
 *   N6  reduced-motion：着陆页不再启动 LightRays 渲染循环
 *
 * 运行前提：npm run dev（5174）。截图输出到 gui-test-screenshots/h_*.png
 */
let chromium;
try {
  ({ chromium } = require("playwright"));
} catch {
  ({ chromium } = require(process.env.PLAYWRIGHT_PATH || "/home/kirky/.local/lib/node_modules/playwright"));
}

const SHOT = '/home/kirky/projects/CodeNexus/graph-viewer/gui-test-screenshots';
/* dist/e2e_test.lbug 是旧版本 magic 的失效 fixture（引擎报 buffer pool 错误），
 * 用 garrison 真实库验证文件加载链路 */
const SMALL_DB = process.env.GARRISON_LBUG || '/home/kirky/projects/CodeNexus/.codenexus/garrison.lbug';
const results = [];
let pass = 0, fail = 0;
const consoleErrors = [];
const pageErrors = [];

function log(name, ok, detail = '') {
  results.push(`${ok ? 'PASS' : 'FAIL'} — ${name}${detail ? ` — ${detail}` : ''}`);
  ok ? pass++ : fail++;
}

async function shot(page, name) {
  const p = `${SHOT}/${name}.png`;
  await page.screenshot({ path: p });
  console.log(`  shot: ${p}`);
}

(async () => {
  const browser = await chromium.launch({
    headless: true,
    args: ['--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader'],
  });

  /* ── N1: 模式仲裁（demo 污染）────────────────────────── */
  {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
    page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text().slice(0, 200)); });
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    await page.getByRole('button', { name: /Demo Mode|演示模式/ }).click();
    await page.waitForTimeout(2000);
    await page.locator('header').getByText('CodeNexus').click(); /* 返回着陆页 */
    await page.waitForTimeout(500);
    await page.locator('input[type="file"]').first().setInputFiles(SMALL_DB);
    await page.waitForTimeout(12000);
    const header = await page.locator('header').innerText();
    const polluted = /GraphIndex|QueryEngine|StorageDb/.test(header) ||
      /\b25 nodes \/ 25 edges\b/.test(header);
    const sampled = /\d+ \/ [\d,]+ nodes/.test(header); /* 采样透明化：shown / total */
    log('N1 demo→file 模式仲裁', header.includes('garrison.lbug') && !polluted,
      `header: ${header.replace(/\n/g, ' | ').slice(0, 120)}`);
    log('N1b 真实库采样比例展示', sampled, `sampled format=${sampled}`);
    await shot(page, 'h_n1_mode_arbitration');
    await page.context().close();
  }

  /* ── N2: 筛选空集语义 ────────────────────────────────── */
  {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    await page.getByRole('button', { name: /Demo Mode|演示模式/ }).click();
    await page.waitForTimeout(2000);
    await page.getByText('None', { exact: true }).click();
    await page.waitForTimeout(600);
    const h1 = await page.locator('header').innerText();
    const emptied = /0 \/ 25 nodes \/ 0 edges/.test(h1) || /0 nodes \/ 0 edges/.test(h1);
    const resetBtn = page.getByRole('button', { name: /Reset Filters|重置筛选/ });
    const resetVisible = await resetBtn.count();
    let restored = false;
    if (resetVisible) {
      await resetBtn.click();
      await page.waitForTimeout(600);
      const h2 = await page.locator('header').innerText();
      restored = /25 nodes \/ 25 edges/.test(h2);
    }
    log('N2 None 真正清空', emptied, `header: ${h1.replace(/\n/g, ' | ').slice(0, 100)}`);
    log('N2 Reset Filters 恢复', restored && resetVisible > 0, `reset btn=${resetVisible}, restored=${restored}`);
    /* N2b demo 模式下节点上限无意义 → 应禁用 */
    const limitDisabled = await page.locator('header select').isDisabled();
    log('N2b demo 下节点上限禁用', limitDisabled, `disabled=${limitDisabled}`);
    await shot(page, 'h_n2_none_filter');
    await page.context().close();
  }

  /* ── N3: 小屏抽屉 ────────────────────────────────────── */
  {
    const page = await (await browser.newContext({ viewport: { width: 390, height: 844 } })).newPage();
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    await page.getByRole('button', { name: /Demo Mode|演示模式/ }).click();
    await page.waitForTimeout(2000);
    const toggle = page.getByRole('button', { name: /Filters|筛选/ });
    const toggleVisible = await toggle.isVisible();
    const panel = page.locator('#cn-filter-panel');
    const boxClosed = await panel.boundingBox();
    await toggle.click();
    /* SwiftShader 软件渲染下 CSS 转场慢 — 轮询等待滑入完成 */
    let boxOpen = null;
    for (let i = 0; i < 10; i++) {
      await page.waitForTimeout(300);
      boxOpen = await panel.boundingBox();
      if (boxOpen && Math.abs(boxOpen.x) < 1) break;
    }
    await shot(page, 'h_n3_drawer_open');
    await page.mouse.click(390 - 10, 500); /* 点遮罩区域（右侧）关闭 */
    let boxReClosed = null;
    for (let i = 0; i < 10; i++) {
      await page.waitForTimeout(300);
      boxReClosed = await panel.boundingBox();
      if (boxReClosed && boxReClosed.x < -200) break;
    }
    const shapeOk = boxClosed && boxOpen && boxReClosed &&
      boxClosed.x < 0 && Math.abs(boxOpen.x) < 1 && boxReClosed.x < 0;
    log('N3 小屏筛选抽屉', toggleVisible && shapeOk,
      `toggle=${toggleVisible}, closed.x=${boxClosed?.x}, open.x=${boxOpen?.x}, reclosed.x=${boxReClosed?.x}`);
    /* N3b Escape 关闭抽屉 */
    await toggle.click();
    await page.waitForTimeout(800);
    await page.keyboard.press('Escape');
    let boxEsc = null;
    for (let i = 0; i < 10; i++) {
      await page.waitForTimeout(300);
      boxEsc = await panel.boundingBox();
      if (boxEsc && boxEsc.x < -200) break;
    }
    log('N3b Escape 关闭抽屉', !!boxEsc && boxEsc.x < -200, `esc.x=${boxEsc?.x}`);
    /* N3c 关闭态面板 visibility:hidden → 不可聚焦、不在 Tab 序内。
     * visibility 是离散过渡属性，翻转发生在 transition 结束瞬间，
     * 需轮询等到真正 hidden 再断言 focus 被拒绝 */
    let hidden = false;
    for (let i = 0; i < 10; i++) {
      await page.waitForTimeout(200);
      hidden = await page.evaluate(() =>
        getComputedStyle(document.querySelector('#cn-filter-panel')).visibility === 'hidden');
      if (hidden) break;
    }
    await page.evaluate(() => document.querySelector('#cn-filter-panel button')?.focus());
    const focusInside = await page.evaluate(() =>
      document.querySelector('#cn-filter-panel')?.contains(document.activeElement) ?? false);
    log('N3c 关闭态不可聚焦', hidden && focusInside === false, `visibilityHidden=${hidden}, focusInside=${focusInside}`);
    await page.context().close();
  }

  /* ── N4: i18n 持久化 ─────────────────────────────────── */
  {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    await page.getByText('中', { exact: true }).first().click();
    await page.waitForTimeout(400);
    await page.reload();
    await page.waitForLoadState('domcontentloaded');
    await page.waitForTimeout(800);
    const body = await page.locator('body').innerText();
    log('N4 语言偏好刷新保持', /演示模式/.test(body), `zh kept=${/演示模式/.test(body)}`);
    await page.context().close();
  }

  /* ── N5: 键盘可达拖放区 ──────────────────────────────── */
  {
    const page = await (await browser.newContext({ viewport: { width: 1440, height: 900 } })).newPage();
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    /* 纯键盘路径：Tab 前进直到焦点落在拖放区按钮上，再 Enter 触发文件选择 */
    let focusedText = '';
    for (let i = 0; i < 8; i++) {
      await page.keyboard.press('Tab');
      focusedText = await page.evaluate(() => document.activeElement?.textContent ?? '');
      if (/Drop .lbug|拖放 .lbug/.test(focusedText)) break;
    }
    /* 偶发 filechooser 事件丢失（套件内连续上下文切换），允许一次重试 */
    let chooserOpened = false;
    for (let attempt = 0; attempt < 2 && !chooserOpened; attempt++) {
      const chooserPromise = page.waitForEvent('filechooser', { timeout: 4000 });
      await page.keyboard.press('Enter');
      try { await chooserPromise; chooserOpened = true; } catch { /* 重试 */ }
    }
    log('N5 拖放区键盘可达', /Drop .lbug|拖放 .lbug/.test(focusedText) && chooserOpened,
      `focused="${focusedText.slice(0, 40)}", chooser=${chooserOpened}`);
    await page.context().close();
  }

  /* ── N6: reduced-motion 下 LightRays 不启动 ──────────── */
  {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: 'reduce' });
    const page = await ctx.newPage();
    page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));
    await page.goto('http://localhost:5174/', { timeout: 15000 });
    await page.waitForLoadState('domcontentloaded');
    await page.waitForTimeout(1500);
    const raysCanvas = await page.locator('.light-rays-container canvas').count();
    log('N6 reduced-motion 关闭光线动画', raysCanvas === 0, `light-rays canvas=${raysCanvas}`);
    await shot(page, 'h_n6_reduced_motion');
    await page.context().close();
  }

  await browser.close();
  console.log('\n===== harden_verify =====');
  for (const r of results) console.log(r);
  console.log(`===== RESULT: ${pass} passed, ${fail} failed =====`);
  console.log('console errors:', JSON.stringify(consoleErrors));
  console.log('page errors:', JSON.stringify(pageErrors));
  process.exit(fail > 0 ? 1 : 0);
})().catch((e) => { console.error('FATAL', e); process.exit(1); });
