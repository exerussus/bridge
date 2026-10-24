<script lang="ts">
  import { closeModal, toast, ui, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  let { id = null, fd, onsaved }: { id?: string | null; fd: any; onsaved: () => void } = $props();
  const f = id ? fd.own.find((x: any) => x.id === id) : { id: '', title: '', path: '', access: {} };
  let fid = $state(f.id);
  let title = $state(f.title || '');
  let path = $state(f.path || '');
  let acc = $state<Record<string, string>>({ ...(f.access || {}) });
  const groups = Object.entries(ui.ov.groups || {}) as [string, any][];
  const peers = ui.ov.peers.filter((p: any) => !p.invite);
  const OPTS: [string, string][] = [['', 'нет доступа'], ['read', 'чтение'], ['write', 'запись']];
  async function save() {
    const access: Record<string, string> = {};
    for (const [k, v] of Object.entries(acc)) if (v) access[k] = v;
    try {
      const r = await api('POST', 'folders', { id: fid.trim(), title: title.trim(), path: path.trim(), access });
      closeModal();
      toast(r.saved ? 'Сохранено' : 'Применено, но bridge.json не записан: ' + r.error, r.saved ? 'ok' : 'bad');
      onsaved();
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>{id ? 'Папка «' + id + '»' : 'Новая общая папка'}</h3>
{#if id}
  <label class="f">Короткое имя (латиница, цифры, - и _; потом не меняется)<input value={fid} disabled /></label>
  <label class="f">Название для людей<input bind:value={title} use:focus placeholder="Обмен с технологом" /></label>
{:else}
  <label class="f">Короткое имя (латиница, цифры, - и _; потом не меняется)<input bind:value={fid} use:focus placeholder="obmen" /></label>
  <label class="f">Название для людей<input bind:value={title} placeholder="Обмен с технологом" /></label>
{/if}
<label class="f">Где лежит на этом ПК (пусто — общие-мои\имя)<input bind:value={path} placeholder="D:\Проект\Обмен" /></label>
<div class="small muted" style="margin-bottom:6px">Кому доступ{groups.length ? ' — группе целиком или отдельным соседям (у соседа действует большее из прав)' : ''}</div>
<table><tbody>
  {#each groups as [g, v]}
    <tr><td>👥 <b>{v.title || g}</b> <span class="small muted">группа: {v.members.map(titleOf).join(', ') || 'пусто'}</span></td>
      <td><select bind:value={acc['@' + g]}>{#each OPTS as [a, t]}<option value={a}>{t}</option>{/each}</select></td></tr>
  {/each}
  {#each peers as p (p.name)}
    <tr><td>{p.title || p.name} <span class="small muted">{p.name}</span></td>
      <td><select bind:value={acc[p.name]}>{#each OPTS as [a, t]}<option value={a}>{t}</option>{/each}</select></td></tr>
  {/each}
  <tr><td>Все остальные соседи <span class="small muted">в том числе будущие</span></td>
    <td><select bind:value={acc['*']}>{#each OPTS as [a, t]}<option value={a}>{t}</option>{/each}</select></td></tr>
</tbody></table>
<div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>{id ? 'Сохранить' : 'Создать'}</button></div>
