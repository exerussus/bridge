<script lang="ts">
  import { api } from '../lib/api';
  import Gi from '../components/Gi.svelte';
  let lines = $state<string[]>([]);
  let q = $state('');
  let live = $state(true);
  let box = $state<HTMLElement>();
  async function refresh() {
    try {
      const r = await api('GET', 'log?lines=500');
      const atBottom = !box || box.scrollTop + box.clientHeight >= box.scrollHeight - 20;
      lines = r.lines;
      if (atBottom) requestAnimationFrame(() => { if (box) box.scrollTop = box.scrollHeight; });
    } catch {}
  }
  $effect(() => {
    refresh();
    const t = setInterval(() => { if (live) refresh(); }, 2000);
    return () => clearInterval(t);
  });
  const cls = (l: string) => (/Отказ|Ошибка|ошибк|не удалось|нет связи/i.test(l) ? 'e' : /ЖДЁТ|конфликт/i.test(l) ? 'w' : /доставлено|получено|выполнено/i.test(l) ? 's' : '');
  const shown = $derived(lines.filter(l => !q || l.toLowerCase().includes(q.toLowerCase())));
</script>

<div class="card">
  <div class="row" style="margin-bottom:8px"><h3 style="margin:0">Журнал узла</h3><div class="grow"></div>
    <input bind:value={q} placeholder="фильтр" /> <label class="small"><input type="checkbox" bind:checked={live} /> обновлять</label>
    <button class="b sm" data-open="data"><Gi n="folder" /> Данные узла</button></div>
  <div class="log" bind:this={box}>{#each shown as l, i (i)}<div class={cls(l)}>{l}</div>{/each}</div>
</div>
