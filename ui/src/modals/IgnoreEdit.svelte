<script lang="ts">
  import { closeModal, toast, peerTitle } from '../lib/state.svelte';
  import { igChange, localMatch } from '../lib/ignore';
  import Gi from '../components/Gi.svelte';
  let { scope, peer = '', id = '', fd, ig, onsaved }: { scope: string; peer?: string; id?: string; fd: any; ig: any; onsaved: () => void } = $props();
  let own: string[] = [], owner: string[] = [], title = 'Игнор всего узла';
  if (scope === 'node') own = ig ? ig.node : [];
  if (scope === 'folder') { const f = fd.own.find((x: any) => x.id === id); own = f.ignore || []; title = 'Игнор папки «' + (f.title || id) + '»'; }
  if (scope === 'mount') { const m = fd.mounts.find((x: any) => x.peer === peer && x.id === id); own = m.ignore || []; owner = m.ownerIgnore || []; title = 'Игнор папки «' + (m.title || id) + '» у ' + peerTitle(peer); }
  let text = $state(own.join('\n'));
  let test = $state('');
  const hit = $derived(test.trim() ? localMatch(text.split('\n').concat(owner), test.trim()) : null);
  async function save() {
    try { await igChange('set', { scope, peer, id, patterns: text.split('\n').map(x => x.trim()).filter(Boolean) }); closeModal(); toast('Игнор сохранён', 'ok'); onsaved(); } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLTextAreaElement) { n.focus(); }
</script>

<h3><Gi n="block" /> {title}</h3>
<div class="small muted" style="margin-bottom:8px">{scope === 'folder' ? 'Эти файлы и папки не увидит никто из подключённых к папке соседей, и положить их туда будет нельзя.' : scope === 'mount' ? 'Только у вас: такие файлы не скачиваются и не отправляются хозяину. Ваши локальные файлы не трогаются.' : 'Действует на общие папки, ручную отправку и пространства.'}</div>
{#if owner.length}<div class="small muted">Игнор хозяина (менять может только он):</div><div style="margin:4px 0 10px">{#each owner as p}<span class="chip ig">{p}</span>{/each}</div>{/if}
<label class="f">Шаблоны — по одному в строке<textarea bind:value={text} use:focus style="min-height:160px;font-family:var(--mono)"></textarea></label>
<ul class="small muted" style="margin:0 0 10px;padding-left:18px">{#each ig ? ig.syntax : [] as x}<li>{x}</li>{/each}</ul>
<div class="row"><input bind:value={test} placeholder="проверить путь" style="flex:1" />
  <span class="small">{#if test.trim()}{#if hit}<span style="color:var(--warn)">скрыт: {hit}</span>{:else}<span style="color:var(--ok)">виден</span>{/if}{/if}</span></div>
<div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>Сохранить</button></div>
