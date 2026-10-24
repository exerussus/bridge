<script lang="ts">
  import { closeModal, toast, ui, load } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { translit } from '../lib/util';
  import { shareDel } from '../lib/shares';
  let { id = null, preset = null }: { id?: string | null; preset?: any } = $props();
  const cur = id ? ui.ov.shares.find((x: any) => x.id === id) : null;
  const c = cur
    ? { id: cur.id, title: cur.title || '', folder: cur.folderCfg || '', peers: cur.peers || [], accept: cur.accept || [], recordsAccept: cur.recordsAccept || [] }
    : Object.assign({ id: '', title: '', folder: '', peers: [], accept: [{ pattern: '**', from: ['*'] }], recordsAccept: [] }, preset || {});
  const peers = ui.ov.peers.filter((p: any) => !p.invite && !p.pending);
  const groups = Object.entries(ui.ov.groups || {}) as [string, any][];
  let title = $state(c.title);
  let sid = $state(c.id);
  let touched = !!id;
  let folder = $state(c.folder);
  let members = $state<string[]>([...c.peers]);
  let rules = $state<{ pattern: string; from: string }[]>(c.accept.map((a: any) => ({ pattern: a.pattern, from: (a.from || ['*'])[0] })));
  let recMode = $state(c.recordsAccept.length ? 'some' : 'all');
  let recFrom = $state<string[]>(peers.filter((p: any) => c.recordsAccept.some((r: any) => (r.from || []).includes(p.name) || (r.from || []).includes('*'))).map((p: any) => p.name));
  function onTitle() { if (!touched) sid = translit(title); }
  async function save() {
    const s = (sid.trim() || translit(title)).trim();
    const accept = rules.map(r => ({ pattern: r.pattern.trim(), from: [r.from] })).filter(a => a.pattern);
    const star = accept.findIndex(x => x.pattern === '**');
    if (star >= 0 && star < accept.length - 1) { toast('Правило «**» ловит всё — правила ниже него не сработают. Поставьте его последним', 'bad'); return; }
    if (recMode === 'some' && !recFrom.length) { toast('Отметьте, от кого принимать записи', 'bad'); return; }
    const body = { id: s, title: title.trim(), folder: folder.trim(), peers: members, accept, recordsAccept: recMode === 'some' ? [{ from: recFrom, types: ['*'], ops: ['*'] }] : [] };
    try {
      const r = await api('POST', 'shares', body);
      closeModal();
      toast(r.created ? 'Пространство создано — участники увидят предложение подключить его' : 'Сохранено', 'ok');
      await load();
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>{id ? 'Пространство «' + (c.title || id) + '»' : 'Новое пространство'}</h3>
<div class="row" style="gap:10px">
  <label class="f" style="flex:1">Название<input bind:value={title} oninput={onTitle} use:focus placeholder="Проект" /></label>
  <label class="f" style="width:200px">Id (латиницей)<input bind:value={sid} oninput={() => (touched = true)} disabled={!!id} placeholder="project" /></label>
</div>
<label class="f">Папка на этом ПК<input class="mono" bind:value={folder} placeholder="Пространства/<id> — рядом с мостом, или полный путь" /></label>
<div class="f">Кто участвует<div class="row" style="flex-wrap:wrap">
  {#each groups as [g, v]}<label class="chip"><input type="checkbox" value={'@' + g} bind:group={members} /> 👥 {v.title || g}</label>{/each}
  {#each peers as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={members} /> {p.title || p.name}</label>{/each}
</div></div>
<div class="f">Какие файлы принимать от участников <span class="small muted">(первое подходящее правило; пусто — только раздаю)</span>
  <div style="margin-top:6px">
    {#each rules as r, i}
      <div class="row srule" style="gap:6px;margin-bottom:6px">
        <input class="mono" bind:value={r.pattern} placeholder="задания/**" style="flex:1" />
        <select bind:value={r.from}><option value="*">от всех</option>{#each peers as p (p.name)}<option value={p.name}>от {p.title || p.name}</option>{/each}</select>
        <button class="b sm ib" type="button" title="Убрать правило" onclick={() => rules.splice(i, 1)}>✕</button>
      </div>
    {/each}
  </div>
  <div class="row" style="gap:6px">
    <button class="b sm" type="button" onclick={() => rules.push({ pattern: '', from: '*' })}>＋ Правило</button>
    <button class="b sm" type="button" onclick={() => (rules = [{ pattern: '**', from: '*' }])}>Всё от всех</button>
    <button class="b sm" type="button" onclick={() => (rules = [])}>Ничего</button>
    <span class="small muted">шаблоны: <code>**</code> — всё, <code>отчёты/**</code> — папка, <code>*.md</code></span>
  </div>
</div>
<div class="f">Записи (задачи, решения, заметки)<div class="row" style="gap:12px;flex-wrap:wrap">
  <label class="small"><input type="radio" value="all" bind:group={recMode} /> принимать от всех участников</label>
  <label class="small"><input type="radio" value="some" bind:group={recMode} /> только от отмеченных:</label>
  {#each peers as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={recFrom} /> {p.title || p.name}</label>{/each}
</div></div>
<div class="small muted">У каждого участника должно быть пространство с тем же id — им придёт предложение подключить его у себя. {id ? 'Сменили папку — пространство откроется в новой папке, старые файлы останутся на месте.' : ''}</div>
<div class="row" style="margin-top:12px">
  {#if id}<button class="b danger" onclick={() => shareDel(id!)}>Убрать пространство</button>{/if}
  <div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>{id ? 'Сохранить' : 'Создать'}</button>
</div>
