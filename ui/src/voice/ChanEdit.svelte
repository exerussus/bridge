<script lang="ts">
  import { closeModal, toast, ui } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { translit } from '../lib/util';
  import { CH, loadChannels } from './engine.svelte';
  let { id = null }: { id?: string | null } = $props();
  const c = id ? CH.own.find(x => x.id === id) || { id, title: '', members: [] } : { id: '', title: '', members: ['*'] };
  let title = $state(c.title || '');
  let cid = $state(c.id);
  let members = $state<string[]>([...c.members]);
  async function save() {
    const key = cid.trim() || translit(title.trim()) || 'kanal';
    try { await api('POST', 'channels', { id: key, title: title.trim(), members }); closeModal(); toast('Канал сохранён — соседи увидят его через несколько секунд', 'ok'); loadChannels(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function del() {
    if (!confirm('Удалить канал «' + (c.title || id) + '»?')) return;
    try { await api('POST', 'channels/remove?id=' + encodeURIComponent(id!)); closeModal(); loadChannels(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLInputElement) { n.focus(); }

  // ── оформление (только у существующего своего канала): иконка и подложка ──
  import { pickFiles } from '../lib/util';
  import { me } from '../lib/state.svelte';
  import { CART, artImg } from '../lib/chanart.svelte';
  import { TILE_DEFAULT, type TileStyle } from '../lib/tile.svelte';
  import TileArt from '../components/TileArt.svelte';
  let art = $state<{ icon: string; back: string; style: TileStyle }>({ icon: '', back: '', style: {} });
  const chref = { owner: me(), id: id || '' };
  $effect(() => { if (id) api('GET', 'chan/art?room=' + encodeURIComponent(me() + ':' + id)).then(r => (art = { icon: r.icon || '', back: r.back || '', style: r.style || {} })).catch(() => {}); });
  const ST = $derived({ ...TILE_DEFAULT, ...art.style });
  async function refreshArt() { delete CART[me() + ':' + id]; await loadChannels(); }
  async function pick(what: 'icon' | 'back') {
    const [f] = await pickFiles({ accept: 'image/*' });
    if (!f) return;
    try {
      let body: Blob = f;
      const gif = f.type === 'image/gif' || /\.gif$/i.test(f.name), max = what === 'icon' ? 2 : 4;
      if (gif) { if (f.size > max * 1048576) throw new Error('гифка больше ' + max + ' МБ'); }
      else {
        // иконка — квадрат 256 по центру, подложка — до 1600 px
        const bmp = await createImageBitmap(f), c = document.createElement('canvas');
        if (what === 'icon') { const side = Math.min(bmp.width, bmp.height); c.width = c.height = 256; c.getContext('2d')!.drawImage(bmp, (bmp.width - side) / 2, (bmp.height - side) / 2, side, side, 0, 0, 256, 256); }
        else { const k = Math.min(1, 1600 / Math.max(bmp.width, bmp.height)); c.width = Math.round(bmp.width * k); c.height = Math.round(bmp.height * k); c.getContext('2d')!.drawImage(bmp, 0, 0, c.width, c.height); }
        body = await new Promise<Blob>(r => c.toBlob(b => r(b!), /png|webp/.test(f.type) ? 'image/png' : 'image/jpeg', 0.9));
      }
      const r = await api('POST', 'chan/img?' + new URLSearchParams({ id: id!, what }), body, true);
      art[what] = r.hash; await refreshArt();
    } catch (e: any) { toast((what === 'icon' ? 'Иконка: ' : 'Подложка: ') + e.message, 'bad'); }
  }
  async function drop(what: 'icon' | 'back') { try { await api('POST', 'chan/img/remove', { id, what }); art[what] = ''; await refreshArt(); } catch (e: any) { toast(e.message, 'bad'); } }
  let saveT = 0;
  function setSt(k: keyof TileStyle, v: any) {
    art.style = { ...art.style, [k]: v };
    clearTimeout(saveT);
    saveT = setTimeout(async () => { try { await api('POST', 'chan/style', { id, style: $state.snapshot(art.style) }); await refreshArt(); } catch (e: any) { toast(e.message, 'bad'); } }, 350) as unknown as number;
  }
  const MASKS: [string, string][] = [['none', 'нет'], ['bottom', 'книзу'], ['top', 'кверху'], ['sides', 'по бокам'], ['vignette', 'виньетка'], ['diagonal', 'диагональ']];
</script>

<h3>{id ? 'Канал «' + (c.title || id) + '»' : 'Новый канал'}</h3>
<label class="f">Название<input bind:value={title} use:focus placeholder="Семья" /></label>
<label class="f">Короткое имя (латиницей)<input bind:value={cid} disabled={!!id} placeholder="family" /></label>
<div class="f">Кто может зайти<div class="row">
  <label class="chip"><input type="checkbox" value="*" bind:group={members} /> все соседи</label>
  {#each Object.entries(ui.ov.groups || {}) as [g, v]}<label class="chip"><input type="checkbox" value={'@' + g} bind:group={members} /> 👥 {(v as any).title || g}</label>{/each}
  {#each ui.ov.peers.filter((p: any) => !p.invite && !p.pending) as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={members} /> {p.title || p.name}</label>{/each}
</div></div>
{#if id}
  <div class="f">Оформление <span class="muted">· видят все участники</span>
    <div class="chart">
      <div class="chan cart chprev">{#if art.back}<TileArt st={art.style} url={artImg(chref, 'back', art.back)} />{/if}<div class="row" style="gap:8px">{#if art.icon}<img class="cicon" alt="" src={artImg(chref, 'icon', art.icon)} />{/if}<b class="cname">{title || c.title || id}</b></div><span class="small muted">так канал выглядит в списке</span></div>
      <div class="chctl">
        <div class="row" style="gap:6px"><span class="lbl">Иконка</span><button class="b sm" onclick={() => pick('icon')}>{art.icon ? 'Сменить…' : 'Выбрать…'}</button><button class="b sm" disabled={!art.icon} onclick={() => drop('icon')}>Убрать</button><span class="small muted">картинка или гифка до 2 МБ</span></div>
        <div class="row" style="gap:6px"><span class="lbl">Подложка</span><button class="b sm" onclick={() => pick('back')}>{art.back ? 'Сменить…' : 'Выбрать…'}</button><button class="b sm" disabled={!art.back} onclick={() => drop('back')}>Убрать</button><span class="small muted">до 4 МБ</span></div>
        <div class="row" style="gap:6px"><span class="lbl">Маска</span><div class="seg">{#each MASKS as [v, t]}<button class:on={ST.mask === v} disabled={!art.back} onclick={() => setSt('mask', v)}>{t}</button>{/each}</div></div>
        <div class="row" style="gap:6px"><span class="lbl">Прозрачность</span><input type="range" min="5" max="100" value={ST.opacity} disabled={!art.back} oninput={e => setSt('opacity', +(e.target as HTMLInputElement).value)} style="flex:1" /><em class="tval">{ST.opacity}%</em></div>
        <div class="row" style="gap:6px"><span class="lbl">Положение ↕</span><input type="range" min="0" max="100" value={ST.y} disabled={!art.back} oninput={e => setSt('y', +(e.target as HTMLInputElement).value)} style="flex:1" /><em class="tval">{ST.y}%</em></div>
        <div class="row" style="gap:6px"><span class="lbl">Размытие</span><input type="range" min="0" max="20" value={ST.blur} disabled={!art.back} oninput={e => setSt('blur', +(e.target as HTMLInputElement).value)} style="flex:1" /><em class="tval">{ST.blur} px</em></div>
      </div>
    </div>
  </div>
{/if}
<div class="small muted">Канал живёт на этом ПК: соседи из списка увидят его у себя и смогут заходить. Если двое участников не соседи друг другу, сигналы между ними передаст этот ПК (звук и видео всё равно идут напрямую).</div>
<div class="row" style="margin-top:12px">{#if id}<button class="b danger" onclick={del}>Удалить канал</button>{/if}<div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>Сохранить</button></div>
