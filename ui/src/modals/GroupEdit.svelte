<script lang="ts">
  import { closeModal, toast, ui, load } from '../lib/state.svelte';
  import { api } from '../lib/api';
  let { id = null, onsaved }: { id?: string | null; onsaved: () => void } = $props();
  const g = id ? ui.ov.groups[id] : { title: '', members: [] };
  let title = $state(g.title || '');
  let gid = $state(id || '');
  let members = $state<string[]>([...g.members]);
  async function save() {
    const key = id || gid.trim() || title.trim().toLowerCase().replace(/[^a-z0-9_-]+/g, '-');
    try { await api('POST', 'groups', { id: key, title: title.trim(), members }); closeModal(); await load(); onsaved(); toast('Группа сохранена', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
  }
</script>

<h3>{id ? 'Группа «' + (g.title || id) + '»' : 'Новая группа'}</h3>
<label class="f">Название — для вас<input bind:value={title} placeholder="Семья" /></label>
<label class="f">Короткое имя (латиницей)<input bind:value={gid} disabled={!!id} placeholder="family" /></label>
<div class="f">Кто в группе<div class="row">{#each ui.ov.peers.filter((p: any) => !p.invite) as p (p.name)}<label class="chip"><input type="checkbox" value={p.name} bind:group={members} /> {p.title || p.name}</label>{/each}</div></div>
<div class="small muted">Дать группе доступ к папке — во вкладке «Общие папки» (кнопка доступа у папки) или щелчком по клетке таблицы здесь.</div>
<div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>Сохранить</button></div>
