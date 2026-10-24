<script lang="ts">
  // «Поделиться версией»: соседям со старой версией уходит предложение, обновляются они только с их подтверждения.
  import { closeModal, toast, ui, titleOf } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { vcmp } from '../lib/util';
  const mine = ui.ov.node.version;
  const list = ui.ov.peers.filter((p: any) => p.enabled && !p.invite);
  const why = (p: any) => (!p.online ? 'не на связи' : !p.version ? 'версия неизвестна' : vcmp(p.version, mine) >= 0 ? 'уже ' + p.version : '');
  let picked = $state<string[]>(list.filter((p: any) => !why(p)).map((p: any) => p.name));
  let res = $state<Record<string, string>>({});
  let busy = $state(false);
  async function go() {
    if (!picked.length) return;
    busy = true;
    try { const r = await api('POST', 'update/offer', { peers: picked }); res = r.results || {}; toast('Предложение отправлено', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
</script>

<div class="row"><h3 style="margin:0">Поделиться версией</h3><div class="grow"></div><span class="pill acc">v{mine}</span></div>
<div class="small muted" style="margin:6px 0 10px">Соседям уйдёт предложение обновиться. У каждого спросят подтверждение; после «Обновить» мост скачает новые файлы отсюда, проверит их, заменит и перезапустится. Мостам до 2.12 файлы придут обычной отправкой — заменить их нужно один раз вручную.</div>
<table class="tbl upd"><tbody>
  {#each list as p (p.name)}
    {@const w = why(p)}
    <tr><td style="width:28px"><input type="checkbox" value={p.name} bind:group={picked} disabled={!!w} /></td><td>{titleOf(p.name)}</td>
      <td class="mono small" style="width:70px">{p.version || '—'}</td><td class="small updres" class:muted={!res[p.name]}>{res[p.name] || w || 'можно обновить'}</td></tr>
  {:else}<tr><td class="muted">Соседей нет</td></tr>{/each}
</tbody></table>
<div class="row" style="margin-top:12px"><div class="grow"></div><button class="b" onclick={closeModal}>Закрыть</button><button class="b pri" style="min-width:130px" disabled={!picked.length || busy} onclick={go}>{busy ? 'Отправляю…' : 'Предложить'}</button></div>
