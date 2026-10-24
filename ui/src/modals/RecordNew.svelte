<script lang="ts">
  import { closeModal, toast, ui, peerTitle } from '../lib/state.svelte';
  import { act } from '../lib/api';
  import { TYPE_TITLE, REC_TYPES, kvRead, type KV } from '../lib/records';
  import KvRows from '../components/KvRows.svelte';
  import { api } from '../lib/api';
  import { agentPeers, taskPrompt, delegate } from '../lib/agenttask';
  // «Решает агент»: задача сразу поручается Claude на ПК агента (он же исполнитель — статус ведёт сам)
  let agents = $state<{ name: string; mode: string }[]>([]);
  let agent = $state('');
  $effect(() => { agentPeers().then(p => (agents = p.filter(x => who.includes(x.name)))); });
  let { share, oncreated }: { share: string; oncreated: (id: string) => void } = $props();
  const sh = ui.ov.shares.find((s: any) => s.id === share);
  const self = ui.ov.node.node;
  const who = [self, ...sh.peers.filter((p: string) => !p.startsWith('@'))];
  let kind = $state('task');
  let custom = $state('');
  let to = $state(who[1] || who[0]);
  let title = $state('');
  let body = $state('');
  let ref = $state('');
  let prio = $state('');
  let due = $state('');
  let id = $state('');
  let kv = $state<KV[]>([]);
  const HINT: Record<string, string> = {
    task: 'Путь: новая → принята → в работе → на проверке → готово. Исполнитель принимает и сдаёт, вы закрываете.',
    decision: 'Предложение: участники согласуют или отклоняют.',
    note: 'Просто запись без статусов: заметка, протокол, ссылка.',
    status: 'Состояние чего-то: «сборка зелёная», «стенд свободен» — меняйте его по ходу.',
    custom: 'Свой вид записи без правил статусов: поля — какие нужны.',
  };
  const KINDS: [string, string][] = [['task', 'Задача'], ['decision', 'Решение'], ['note', 'Заметка'], ['status', 'Статус'], ['custom', 'Свой вид…']];
  async function save() {
    const type = kind === 'custom' ? custom.trim() : kind;
    if (!type) { toast('Назовите вид записи', 'bad'); return; }
    const fields: Record<string, any> = Object.assign(kvRead(kv), { title: title.trim() });
    if (!fields.title) { toast('Нужен заголовок', 'bad'); return; }
    if (body.trim()) fields.body = body;
    if (kind === 'task') { fields.assignee = agent || to; if (ref.trim()) fields.ref = ref.trim(); if (prio) fields.priority = prio; if (due) fields.due = due; if (agent) fields.agent = agent; }
    try {
      const r = await act({ action: 'create', type, share, record: id.trim() || null, fields, actor: 'панель' });
      closeModal(); REC_TYPES.add(type);
      toast((TYPE_TITLE[type] || 'Запись') + ' ' + r.record + ' создана', 'ok');
      oncreated(r.record);
      if (kind === 'task' && agent) {
        try { const rec = await api('GET', 'records/' + encodeURIComponent(r.record) + '?share=' + encodeURIComponent(share)); await delegate(share, rec, agent, taskPrompt(share, rec), false); oncreated(r.record); }
        catch (e: any) { toast('Агенту не поручено: ' + e.message, 'bad'); }
      }
    } catch (e: any) { toast(e.message, 'bad'); }
  }
  function focus(n: HTMLInputElement) { n.focus(); }
</script>

<h3>Новая запись · {sh.title || share}</h3>
<div class="f">Вид<div class="seg">{#each KINDS as [v, t]}<button class:on={kind === v} onclick={() => (kind = v)}>{t}</button>{/each}</div></div>
<label class="f" class:hide={kind !== 'custom'}>Название вида (латиницей)<input bind:value={custom} placeholder="bug, idea, meeting…" /></label>
<div class="small muted" style="margin:-4px 0 8px">{HINT[kind]}</div>
<label class="f" class:hide={kind !== 'task'}>Решает агент <span class="muted">· Claude на соседнем ПК сразу возьмётся и сам поведёт статус</span><select bind:value={agent} disabled={!agents.length}><option value="">— нет, человек</option>{#each agents as a}<option value={a.name}>🤖 {peerTitle(a.name)}</option>{/each}</select></label>
<label class="f" class:hide={kind !== 'task'}>Кому<select bind:value={to} disabled={!!agent}>{#each who as w}<option value={w}>{w === self ? 'себе (' + w + ')' : peerTitle(w)}</option>{/each}</select></label>
<label class="f">Заголовок<input bind:value={title} use:focus placeholder="Коротко, о чём" /></label>
<label class="f">Описание<textarea bind:value={body}></textarea></label>
<div class="row" style="gap:10px" class:hide={kind !== 'task'}>
  <label class="f" style="flex:1">Файл задания (необязательно)<input bind:value={ref} placeholder="задания/Z-002-….md" /></label>
  <label class="f" style="width:150px">Приоритет<select bind:value={prio}><option value="">—</option><option>низкий</option><option>обычный</option><option>высокий</option><option>срочно</option></select></label>
  <label class="f" style="width:170px">Срок<input type="date" bind:value={due} /></label>
</div>
<div class="f">Ещё поля <span class="small muted">(необязательно)</span><div><KvRows bind:rows={kv} /></div></div>
<label class="f" style="width:220px">Id (необязательно)<input bind:value={id} placeholder="Z-002" /></label>
<div class="row"><div class="grow"></div><button class="b" onclick={closeModal}>Отмена</button><button class="b pri" onclick={save}>Создать</button></div>
