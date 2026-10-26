<script lang="ts">
  // Заготовки статуса: иконка (цвет или картинка) и текст. Выбирается правой кнопкой на своей плитке в канале.
  import { api, aurl } from '../lib/api';
  import { toast, me } from '../lib/state.svelte';
  import { pickFiles } from '../lib/util';
  import { loadAvatars } from '../voice/engine.svelte';

  const COLORS = ['#5DE08E', '#FFB547', '#FF4F64', '#38E1F0', '#9B7BFF', '#FF8FA0', '#E8EAFB', '#5B6092'];
  let list = $state<any[]>([]);
  let active = $state('');
  let loaded = $state(false);
  let saveT: any = 0;
  const imgUrl = (p: any) => (p.img ? aurl('tile/status/preset/image?id=' + encodeURIComponent(p.id) + '&v=' + p.img) : '');
  $effect(() => { api('GET', 'tile/status/presets').then(r => { list = r.list; active = r.active; loaded = true; }).catch(e => toast('Статусы: ' + e.message, 'bad')); });
  async function after(r: any) { active = r.presets.active; await loadAvatars(); }
  function save(now = false) {
    clearTimeout(saveT);
    const go = async () => { try { const r = await api('POST', 'tile/status/presets', { list: list.map(p => ({ id: p.id, text: p.text, color: p.color || '', silent: !!p.silent })) }); await after(r); } catch (e: any) { toast('Статусы: ' + e.message, 'bad'); } };
    if (now) go(); else saveT = setTimeout(go, 500);
  }
  function add() {
    if (list.length >= 20) return;
    list.push({ id: 's' + Date.now().toString(36), text: '', color: COLORS[list.length % COLORS.length], img: '', silent: false }); save(true);
  }
  function del(i: number) { list.splice(i, 1); save(true); }
  function move(i: number, d: number) { const j = i + d; if (j < 0 || j >= list.length) return; [list[i], list[j]] = [list[j], list[i]]; save(true); }
  async function pick(p: any) {
    const [f] = await pickFiles({ accept: 'image/*' }); if (!f) return;
    try {
      let body: Blob = f;
      if (!(f.type === 'image/gif' || /\.gif$/i.test(f.name))) { const bmp = await createImageBitmap(f), k = Math.min(1, 96 / Math.max(bmp.width, bmp.height)); const c = document.createElement('canvas'); c.width = Math.round(bmp.width * k); c.height = Math.round(bmp.height * k); c.getContext('2d')!.drawImage(bmp, 0, 0, c.width, c.height); body = await new Promise<Blob>(r => c.toBlob(b => r(b!), 'image/png')); }
      else if (f.size > 256 * 1024) throw new Error('гифка больше 256 КБ');
      const r = await api('POST', 'tile/status/preset/image?id=' + encodeURIComponent(p.id), body, true); p.img = r.img;
      if (active === p.id) { await loadAvatars(); }
    } catch (e: any) { toast('Значок: ' + e.message, 'bad'); }
  }
  async function noImg(p: any) { try { await api('POST', 'tile/status/preset/image/remove?id=' + encodeURIComponent(p.id)); p.img = ''; if (active === p.id) { await loadAvatars(); } } catch (e: any) { toast(e.message, 'bad'); } }
  async function apply(id: string) { try { const r = await api('POST', 'tile/status/apply', { id }); await after(r); toast(id ? 'Статус выставлен' : 'Без статуса', 'ok'); } catch (e: any) { toast(e.message, 'bad'); } }
</script>

<div class="small muted" style="margin-bottom:8px">Заготовьте статусы — иконка и текст. В канале щёлкните <b>правой кнопкой по своей плитке</b> и выберите один из них или «Без статуса». Соседи видят выбранный при наведении на вашу плитку. <b>🔕 тишина</b> — пока выбран такой статус, приложение молчит: ни пингов, ни звуков входа, ни звонков, а пинги к вам не проходят (голоса в разговоре — как обычно).</div>
{#if loaded}
  {#each list as p, i (p.id)}
    <div class="stpre" class:on={active === p.id}>
      <span class="sticon" style={p.img ? '' : 'background:' + (p.color || 'transparent')} class:nost={!p.img && !p.color}>{#if p.img}<img src={imgUrl(p)} alt="" />{/if}</span>
      <input class="grow" maxlength="255" placeholder="Например: «на созвоне», «собираю билд», «обед до 14:00»" bind:value={p.text} oninput={() => save()} aria-label="Текст статуса" />
      <span class="stcolors">{#each COLORS as c}<button class="stsw" class:on={p.color === c && !p.img} style="background:{c}" title={c} aria-label={'Цвет ' + c} onclick={() => { p.color = c; if (p.img) noImg(p); save(true); }}></button>{/each}</span>
      <button class="b sm" title="Картинка-значок (можно гифку)" onclick={() => pick(p)}>{p.img ? 'Сменить…' : 'Картинка…'}</button>
      {#if p.img}<button class="b sm" title="Без картинки — цвет" onclick={() => noImg(p)}>✕ карт.</button>{/if}
      <label class="stsilent" title="Полная тишина: пока выбран этот статус, приложение на этом ПК не издаёт ни звука — ни пингов, ни входов, ни звонков; пинги к вам не приходят"><input type="checkbox" bind:checked={p.silent} onchange={() => save(true)} /> 🔕 тишина</label>
      <button class="b sm ib" title="Выше" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
      <button class="b sm ib" title="Выставить сейчас" class:pri={active !== p.id} disabled={active === p.id || !p.text.trim()} onclick={() => apply(p.id)}>{active === p.id ? '✓' : '▶'}</button>
      <button class="b sm ib danger" title="Удалить заготовку" onclick={() => del(i)}>✕</button>
    </div>
  {:else}
    <div class="empty small">Заготовок пока нет</div>
  {/each}
  <div class="row" style="margin-top:8px"><button class="b sm" onclick={add} disabled={list.length >= 20}>＋ Статус</button><div class="grow"></div><button class="b sm" onclick={() => apply('')} disabled={!active}>Без статуса</button></div>
{/if}
