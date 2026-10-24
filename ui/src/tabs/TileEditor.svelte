<script lang="ts">
  // «Плитка»: как вас видят в канале. Картинка-фон (можно гифку) — вписывание, положение, масштаб, маска, мягкость,
  // прозрачность, размытие, оттенок; цвет фона и рамки; аватарка (можно гифку) — форма, размер, обводка; цвет подсветки,
  // когда говорите, и цвет имени. Предпросмотр — та же плитка, что в канале; положение картинки можно тащить мышью.
  import { api } from '../lib/api';
  import { toast, me } from '../lib/state.svelte';
  import { pickFiles } from '../lib/util';
  import { avatarPick, avatarRemove } from '../lib/avatar';
  import { AV, loadAvatars } from '../voice/engine.svelte';
  import { TILE_DEFAULT, TILES, tileImgUrl, stImgUrl, tileVars, tileCls, type TileStyle } from '../lib/tile.svelte';
  import TileArt from '../components/TileArt.svelte';
  import Av from '../components/Av.svelte';

  let st = $state<TileStyle>({});
  let img = $state('');
  let loaded = $state(false);
  let speaking = $state(false);
  let busy = $state(false);
  const S = $derived({ ...TILE_DEFAULT, ...st });
  const url = $derived(tileImgUrl(me(), img));

  $effect(() => {
    api('GET', 'tile').then(r => { st = r.style || {}; img = r.img || ''; stImg = r.stImg || ''; stText = st.stText || ''; loaded = true; }).catch(e => toast('Плитка: ' + e.message, 'bad'));
  });

  // статус: текст и значок (цвет или картинка); видят все в канале при наведении на плитку
  let stText = $state('');
  let stImg = $state('');
  const ST_COLORS = ['#5DE08E', '#FFB547', '#FF4F64', '#38E1F0', '#9B7BFF', '#FF8FA0', '#E8EAFB', '#5B6092'];
  async function stSave(p: { text?: string; color?: string }) {
    try {
      const r = await api('POST', 'tile/status', p);
      const n: any = { ...st }; if (r.status.text) n.stText = r.status.text; else delete n.stText; if (r.status.color) n.stColor = r.status.color; else delete n.stColor; st = n;
      TILES[me()] = { style: n, img, stImg, h: r.hash }; await loadAvatars();
    } catch (e: any) { toast('Статус: ' + e.message, 'bad'); }
  }
  async function stPick() {
    const [f] = await pickFiles({ accept: 'image/*' }); if (!f) return;
    try {
      let body: Blob = f;
      if (!(f.type === 'image/gif' || /\.gif$/i.test(f.name))) { const bmp = await createImageBitmap(f), k = Math.min(1, 96 / Math.max(bmp.width, bmp.height)); const c = document.createElement('canvas'); c.width = Math.round(bmp.width * k); c.height = Math.round(bmp.height * k); c.getContext('2d')!.drawImage(bmp, 0, 0, c.width, c.height); body = await new Promise<Blob>(r => c.toBlob(b => r(b!), 'image/png')); }
      else if (f.size > 256 * 1024) throw new Error('гифка больше 256 КБ');
      const r = await api('POST', 'tile/status/image', body, true); stImg = r.img; TILES[me()] = { style: st, img, stImg, h: r.hash }; await loadAvatars();
    } catch (e: any) { toast('Значок: ' + e.message, 'bad'); }
  }
  async function stDropImg() { await api('POST', 'tile/status/image/remove'); stImg = ''; await stSave({}); }
  async function stClear() { if (stImg) await api('POST', 'tile/status/image/remove'); stImg = ''; stText = ''; await stSave({ text: '', color: '' }); toast('Статус убран'); }

  let saveT = 0;
  function set<K extends keyof TileStyle>(k: K, v: TileStyle[K] | '' | undefined) {
    const n: any = { ...st };
    if (v === '' || v === undefined) delete n[k]; else n[k] = v;
    st = n;
    clearTimeout(saveT);
    saveT = setTimeout(save, 350) as unknown as number;
  }
  async function save() {
    try {
      const r = await api('POST', 'tile', { style: $state.snapshot(st) });
      TILES[me()] = { style: r.style, img, h: r.hash };
      await loadAvatars();
    } catch (e: any) { toast('Плитка: ' + e.message, 'bad'); }
  }
  async function pickImg() {
    const [f] = await pickFiles({ accept: 'image/*' });
    if (!f) return;
    busy = true;
    try {
      let body: Blob = f;
      if (f.type === 'image/gif' || /\.gif$/i.test(f.name)) {
        if (f.size > 4 * 1048576) throw new Error('гифка ' + (f.size / 1048576).toFixed(1) + ' МБ — для плитки нужна до 4 МБ');
      } else {
        // обычная картинка — до 1200 px по длинной стороне (плитке больше не нужно)
        const bmp = await createImageBitmap(f), k = Math.min(1, 1200 / Math.max(bmp.width, bmp.height));
        const c = document.createElement('canvas'); c.width = Math.round(bmp.width * k); c.height = Math.round(bmp.height * k);
        c.getContext('2d')!.drawImage(bmp, 0, 0, c.width, c.height);
        const alpha = /png|webp/.test(f.type);
        body = await new Promise<Blob>(r => c.toBlob(b => r(b!), alpha ? 'image/png' : 'image/jpeg', 0.9));
        if (body.size > 4 * 1048576) body = await new Promise<Blob>(r => c.toBlob(b => r(b!), 'image/jpeg', 0.85));
      }
      const r = await api('POST', 'tile/image', body, true);
      img = r.img;
      await save();
      toast('Картинка плитки обновлена — соседи увидят её через несколько секунд', 'ok');
    } catch (e: any) { toast('Плитка: ' + e.message, 'bad'); }
    busy = false;
  }
  async function dropImg() { try { await api('POST', 'tile/image/remove'); img = ''; await save(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function reset() {
    if (!confirm('Сбросить плитку: убрать картинку и все настройки?')) return;
    try { await api('POST', 'tile/reset'); st = {}; img = ''; delete TILES[me()]; await loadAvatars(); toast('Плитка обычная', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
  }
  // тащить картинку по предпросмотру — точка фокуса (x, y)
  let drag: { x: number; y: number; sx: number; sy: number; w: number; h: number } | null = null;
  function pdown(e: PointerEvent) {
    if (!img) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    drag = { x: S.x, y: S.y, sx: e.clientX, sy: e.clientY, w: r.width, h: r.height };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function pmove(e: PointerEvent) {
    if (!drag) return;
    const k = S.fit === 'repeat' ? 1 : 100 / Math.max(1, S.zoom / 100);
    set('x', Math.round(Math.max(0, Math.min(100, drag.x - ((e.clientX - drag.sx) / drag.w) * k))));
    set('y', Math.round(Math.max(0, Math.min(100, drag.y - ((e.clientY - drag.sy) / drag.h) * k))));
  }
  const MASKS: [string, string][] = [['none', 'нет'], ['bottom', 'книзу'], ['top', 'кверху'], ['sides', 'по бокам'], ['vignette', 'виньетка'], ['circle', 'круг'], ['diagonal', 'диагональ']];
  const SHAPES: [string, string][] = [['circle', 'круг'], ['round', 'скруглённый'], ['square', 'квадрат'], ['hex', 'шестиугольник'], ['none', 'без аватарки']];
  const FITS: [string, string][] = [['cover', 'заполнить'], ['contain', 'целиком'], ['repeat', 'узором']];
</script>

{#snippet seg(k: keyof TileStyle, opts: [string, string][], cur: string)}
  <div class="seg">{#each opts as [v, t]}<button class:on={cur === v} onclick={() => set(k, v as any)}>{t}</button>{/each}</div>
{/snippet}
{#snippet range(k: keyof TileStyle, lbl: string, min: number, max: number, unit: string, dis = false)}
  <div class="setrow"><span>{lbl}</span><span class="row" style="gap:8px"><input type="range" {min} {max} step="1" value={(S as any)[k]} disabled={dis} oninput={e => set(k, +(e.target as HTMLInputElement).value as any)} style="flex:1;min-width:140px" /><em class="tval">{(S as any)[k]}{unit}</em></span><span></span></div>
{/snippet}
{#snippet color(k: keyof TileStyle, lbl: string, hint: string)}
  <div class="setrow"><span>{lbl} <span class="muted">· {hint}</span></span><span class="row" style="gap:8px"><input type="color" value={(st as any)[k] || '#38E1F0'} oninput={e => set(k, (e.target as HTMLInputElement).value.toUpperCase() as any)} /><button class="b sm" disabled={!(st as any)[k]} onclick={() => set(k, '')}>Как обычно</button></span><span></span></div>
{/snippet}

<div class="tiled">
  <div class="tprev">
    <div class="small muted">Так вас видят в канале</div>
    <div class="call one">
      <div class="tile {tileCls(st)}" class:speaking style={tileVars(st)} onpointerdown={pdown} onpointermove={pmove} onpointerup={() => (drag = null)} role="presentation" class:grab={!!img}>
        <TileArt {st} {url} />
        <div class="av"><Av n={me()} title={me()} /></div>
        <b>{me()}</b>
        <div class="inds"><span class="ind">🎙 в разговоре</span></div>
        <span class="small muted tcs"> </span>
        <div class="tact"><button class="b sm ib" tabindex="-1">🔔</button><button class="b sm ib" tabindex="-1">🔊</button></div>
      </div>
    </div>
    <label class="small"><input type="checkbox" bind:checked={speaking} /> показать, как подсвечивается, когда вы говорите</label>
    <div class="small muted">{img ? 'Картинку можно двигать мышью прямо на плитке.' : 'Добавьте картинку или гифку — она ляжет фоном плитки.'}</div>
    <button class="b sm" onclick={reset} disabled={!loaded || (!img && !Object.keys(st).length)}>Сбросить плитку</button>
  </div>
  <div class="tctl">
    <div class="card"><h3>Статус</h3>
      <div class="small muted" style="margin-bottom:8px">Видят все в канале: наведите мышь на плитку — статус покажется рядом.</div>
      <div class="stedit">
        <span class="sticon" style={stImg ? '' : 'background:' + (st.stColor || 'transparent')} class:empty={!stImg && !st.stColor}>{#if stImg}<img src={stImgUrl(me(), stImg)} alt="" />{/if}</span>
        <input class="grow" maxlength="255" placeholder="Чем заняты: «на созвоне до 15:00», «собираю билд»" bind:value={stText} onkeydown={e => { if (e.key === 'Enter') stSave({ text: stText }); }} onblur={() => { if ((st.stText || '') !== stText.trim()) stSave({ text: stText }); }} aria-label="Текст статуса" />
        <span class="small muted stcount">{stText.length}/255</span>
      </div>
      <div class="setrow"><span>Значок</span><span class="row" style="gap:6px;flex-wrap:wrap">{#each ST_COLORS as c}<button class="stsw" class:on={st.stColor === c && !stImg} style="background:{c}" title={c} aria-label={'Цвет ' + c} onclick={() => stSave({ color: c })}></button>{/each}<button class="b sm" onclick={stPick}>{stImg ? 'Сменить картинку…' : 'Картинка…'}</button>{#if stImg}<button class="b sm" onclick={stDropImg}>Без картинки</button>{/if}</span><span></span></div>
      <div class="row" style="justify-content:flex-end"><button class="b sm" onclick={stClear} disabled={!st.stText && !st.stColor && !stImg}>Убрать статус</button></div>
    </div>
    <div class="card"><h3>Картинка плитки</h3>
      <div class="setrow"><span>Фон <span class="muted">· картинка или гифка до 4 МБ</span></span><span class="row" style="gap:8px"><button class="b sm pri" onclick={pickImg} disabled={busy}>{img ? 'Сменить…' : 'Выбрать…'}</button><button class="b sm" onclick={dropImg} disabled={!img || busy}>Убрать</button></span><span></span></div>
      <div class="setrow"><span>Вписать</span>{@render seg('fit', FITS, S.fit)}<span></span></div>
      {@render range('zoom', S.fit === 'repeat' ? 'Размер узора' : 'Масштаб', 100, 400, '%', !img)}
      {@render range('x', 'Положение ↔', 0, 100, '%', !img)}
      {@render range('y', 'Положение ↕', 0, 100, '%', !img)}
      <div class="setrow"><span>Маска</span>{@render seg('mask', MASKS, S.mask)}<span></span></div>
      {@render range('soft', 'Мягкость маски', 0, 100, '%', S.mask === 'none')}
      {@render range('opacity', 'Прозрачность', 5, 100, '%')}
      {@render range('blur', 'Размытие', 0, 20, ' px', !img)}
      {@render color('tint', 'Оттенок', 'перекрашивает картинку')}
      {@render range('tintAmt', 'Сила оттенка', 0, 100, '%', !st.tint)}
    </div>
    <div class="card"><h3>Плитка</h3>
      {@render color('base', 'Цвет плитки', 'под картинкой')}
      {@render color('border', 'Рамка', 'цвет контура')}
      {@render color('glow', 'Подсветка', 'когда вы говорите')}
      {@render color('name', 'Имя', 'цвет подписи')}
    </div>
    <div class="card"><h3>Аватарка</h3>
      <div class="setrow"><span>Картинка <span class="muted">· картинка или гифка до 2 МБ</span></span><span class="row" style="gap:8px"><button class="b sm" onclick={avatarPick}>Сменить…</button><button class="b sm" onclick={avatarRemove} disabled={!AV.me}>Убрать</button></span><span></span></div>
      <div class="setrow"><span>Форма</span>{@render seg('avShape', SHAPES, S.avShape)}<span></span></div>
      {@render range('avSize', 'Размер', 50, 160, '%', S.avShape === 'none')}
      {@render color('avRing', 'Обводка', 'кольцо вокруг аватарки')}
    </div>
  </div>
</div>
