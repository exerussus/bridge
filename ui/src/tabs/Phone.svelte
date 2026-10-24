<script lang="ts">
  import { ui, toast, badgeCount } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { ago, fullTime, copy, pickFiles } from '../lib/util';

  let PH = $state<any>(null);
  let err = $state('');
  let rights = $state('full');
  let pair = $state<any>(null);
  let idx = $state(0);
  let left = $state(0);
  let known: string[] | null = null;
  let to = $state('');
  let sent = $state<string[]>([]);
  let over = $state(false);
  let timer: any = null;

  async function refresh(fresh = false) {
    try { PH = await api('GET', 'mobile' + (fresh ? '?fresh=1' : '')); err = ''; } catch (e: any) { err = e.message; return; }
    const ids = PH.devices.map((d: any) => d.id);
    if (known && pair) { const n = PH.devices.filter((d: any) => !known!.includes(d.id)); if (n.length) { toast('Телефон «' + n[0].name + '» подключён', 'ok'); stopPair(); } }
    known = ids;
    if (!to || !ids.includes(to)) to = ids[0] || '';
    badgeCount('phone', 0);
  }
  $effect(() => { void ui.ov; refresh(); });
  $effect(() => () => clearInterval(timer));

  async function startPair() {
    let p: any;
    try { p = await api('POST', 'devices/pair', { rights }); } catch (e: any) { toast(e.message, 'bad'); return; }
    if (!p.urls.length) { toast('Нет адреса, по которому телефон достучится до ПК: мост слушает только 127.0.0.1', 'bad'); return; }
    pair = p; idx = 0; known = PH ? PH.devices.map((d: any) => d.id) : [];
    const until = Date.now() + p.expiresIn * 1000;
    clearInterval(timer);
    const tick = () => {
      left = Math.max(0, Math.round((until - Date.now()) / 1000));
      if (!left) stopPair(); else if (left % 2 === 0 && ui.tab === 'phone') refresh();
    };
    tick();
    timer = setInterval(tick, 1000);
  }
  function stopPair() { clearInterval(timer); pair = null; }
  const u = $derived(pair ? pair.urls[idx] : null);

  async function setRights(id: string, r: string) { try { await api('POST', 'devices', { id, rights: r }); toast('Доступ изменён', 'ok'); } catch (e: any) { toast(e.message, 'bad'); } refresh(); }
  async function rename(d: any) { const n = prompt('Имя телефона', d.name); if (!n) return; try { await api('POST', 'devices', { id: d.id, name: n }); } catch (e: any) { toast(e.message, 'bad'); } refresh(); }
  async function remove(d: any) {
    if (!confirm('Отключить «' + d.name + '»? Ключ телефона перестанет действовать; подключить снова — по QR-коду.')) return;
    try { await api('POST', 'devices/remove?id=' + encodeURIComponent(d.id)); toast('Отключён', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
    refresh();
  }
  async function send(files: File[]) {
    if (!to) { toast('Сначала подключите телефон', 'bad'); return; }
    for (const f of files) {
      try { const r = await api('POST', 'devices/upload?' + new URLSearchParams({ device: to, path: f.name }), f, true); sent = ['✓ ' + r.path + ' → ' + r.device, ...sent]; }
      catch (e: any) { toast(f.name + ': ' + e.message, 'bad'); }
    }
  }
  async function https(on: boolean, ev?: Event) {
    const b = ev?.currentTarget as HTMLButtonElement | undefined;
    if (b && on) { b.disabled = true; b.textContent = 'Включаю…'; }
    try { const r = await api('POST', 'mobile/https', { on }); if (on && r.ok) toast('HTTPS включён: ' + r.url, 'ok'); if (!on) toast('HTTPS выключен'); } catch (e: any) { toast(e.message, 'bad'); }
    refresh(true);
  }
  function qrSize(n: HTMLElement, _qr?: string) {
    const f = () => { const sv = n.querySelector('svg') as SVGElement | null; if (sv) { sv.style.width = '240px'; sv.style.height = '240px'; } };
    f();
    return { update: f };
  }
</script>

<div class="rooms">
  <div>
    <div class="card"><h3>Подключить телефон</h3>
      {#if !pair}
        <div class="small muted" style="margin-bottom:12px">Телефон откроет мост в браузере. Один раз отсканируйте QR-код — дальше телефон работает с этим ПК сам: отправляет сюда и соседям, получает файлы «на телефон», видит общие папки.</div>
        <label class="f">Доступ телефона<select bind:value={rights}><option value="full">полный — всё, что умеет панель (кроме голоса)</option><option value="files">только файлы</option></select></label>
        <button class="b pri" style="width:100%" onclick={startPair}>Подключить телефон</button>
      {:else}
        <div class="row" style="flex-wrap:wrap;gap:6px;margin-bottom:10px">{#each pair.urls as x, i}<button class="chip" class:w={i === idx} onclick={() => (idx = i)}>{x.label}</button>{/each}</div>
        <div use:qrSize={u?.qr} style="background:#fff;border-radius:14px;padding:10px;display:grid;place-items:center;max-width:280px;margin:0 auto">{@html u?.qr || ''}</div>
        <div class="small muted" style="text-align:center;margin-top:8px">{u?.note || ''}</div>
        <details class="small" style="margin-top:10px"><summary class="muted" style="cursor:pointer">Страница не открывается на телефоне?</summary>
          <ol style="margin:8px 0 0 18px;padding:0;line-height:1.5">
            <li>Телефон в той же сети Wi-Fi, что и ПК (не мобильный интернет, не гостевая Wi-Fi — она отделяет устройства друг от друга).</li>
            <li><b>VPN на телефоне выключен</b> (Happ, v2rayNG и т. п.) — или в нём разрешён обход локальной сети.</li>
            <li>В Windows сеть Wi-Fi/Ethernet — «Частная»: Параметры → Сеть и Интернет → свойства подключения. В «Общедоступной» брандмауэр не пускает телефон.</li>
            <li>Если в журнале моста нет строки «Телефон: открыта страница моста с …» — запрос до ПК не дошёл (пункты 1–3).</li>
            <li>Вне дома — только через Tailscale (справа).</li>
          </ol></details>
        <div style="text-align:center;margin:12px 0 4px"><span class="small muted">или код на телефоне</span>
          <div style="font:600 26px/1.2 var(--display);letter-spacing:.24em;margin-top:2px">{pair.code}</div>
          <span class="small muted">{left ? 'код действует ещё ' + Math.floor(left / 60) + ':' + String(left % 60).padStart(2, '0') : 'код устарел — нажмите «Подключить телефон» ещё раз'}</span></div>
        <div class="row" style="margin-top:8px"><code class="grow small" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{u?.url}</code><button class="b sm" onclick={() => copy(u?.url || '', toast)}>Скопировать</button></div>
        <div class="row" style="margin-top:10px"><div class="grow"></div><button class="b sm" onclick={stopPair}>Готово</button></div>
      {/if}
    </div>
    <div class="card" style="margin-top:14px"><h3>Связь вне дома · Tailscale</h3>
      {#if PH}
        {@const t = PH.tailscale || {}}
        {#if !t.installed}
          <div class="small muted">Дома телефон подключается по Wi-Fi. Чтобы мост работал и вне дома, нужна <b>Tailscale</b> — бесплатная частная сеть между вашими устройствами:</div>
          <ol class="small" style="margin:8px 0 10px 18px;padding:0"><li>Установите Tailscale на этот ПК и на телефон (tailscale.com/download, Google Play).</li><li>Войдите на обоих под одной учётной записью.</li><li>Вернитесь сюда и нажмите «Проверить».</li></ol>
          <button class="b sm" onclick={() => refresh(true)}>Проверить</button>
        {:else if !t.running}
          <div class="row"><span class="pill warn">Tailscale не подключён</span><span class="small muted">{t.state || ''}</span></div>
          <div class="small muted" style="margin:8px 0">Откройте Tailscale на ПК и войдите.</div><button class="b sm" onclick={() => refresh(true)}>Проверить</button>
        {:else}
          <div class="row" style="flex-wrap:wrap"><span class="pill ok">Tailscale на связи</span><code class="small">{t.dns}</code><span class="small muted">{(t.ips || []).join(', ')}</span></div>
          {#if PH.https === 'tailscale' && PH.httpsUrl}
            <div style="margin-top:12px"><span class="pill ok">HTTPS включён</span> <a href={PH.httpsUrl + '/m/'} target="_blank" rel="noopener">{PH.httpsUrl}/m/</a></div>
            <div class="small muted" style="margin:8px 0">Телефон с включённым Tailscale открывает мост по этому адресу и дома, и вне дома. По HTTPS мост можно установить на телефон как приложение — он появится в меню «Поделиться».</div>
            <button class="b sm" onclick={() => https(false)}>Выключить HTTPS</button>
          {:else if PH.needEnable}
            <div class="note" style="margin-top:12px;border-left:2px solid var(--amber);padding:8px 10px;background:var(--warn-soft)">В вашей сети Tailscale выключен HTTPS. Откройте страницу и нажмите «Enable» — это один раз:
              <div class="row" style="margin-top:8px"><a class="b sm pri" href={PH.needEnable} target="_blank" rel="noopener">Открыть страницу Tailscale</a><button class="b sm" onclick={e => https(true, e)}>Проверить снова</button></div></div>
          {:else}
            <div class="small muted" style="margin:10px 0">Включите HTTPS — телефон откроет мост по адресу <code>https://{t.dns}</code>: дома и вне дома, с установкой как приложение и «Поделиться».</div>
            {#if PH.httpsError}<div class="small" style="color:var(--bad);margin-bottom:8px">{PH.httpsError}</div>{/if}
            <button class="b pri sm" onclick={e => https(true, e)}>Включить HTTPS</button>
          {/if}
        {/if}
      {/if}
    </div>
  </div>
  <div>
    <div class="card"><h3>Телефоны</h3>
      {#if err}<div class="empty">{err}</div>
      {:else if PH && PH.devices.length}
        <table><tbody><tr><th>Телефон</th><th>Связь</th><th>Доступ</th><th></th></tr>
          {#each PH.devices as d (d.id)}
            <tr><td><b>{d.name}</b><div class="small muted">подключён {fullTime(d.created)}</div></td>
              <td><span class="pill" class:ok={d.online}>{d.online ? 'на связи' : 'был ' + ago(d.lastSeen)}</span><div class="small muted">{d.lastIp || ''}</div></td>
              <td><select value={d.rights === 'files' ? 'files' : 'full'} onchange={e => setRights(d.id, (e.target as HTMLSelectElement).value)}><option value="full">полный</option><option value="files">только файлы</option></select></td>
              <td style="white-space:nowrap"><button class="b sm" onclick={() => rename(d)}>Имя</button> <button class="b sm" onclick={() => remove(d)}>Отключить</button></td></tr>
          {/each}
        </tbody></table>
      {:else if PH}<div class="empty">Телефонов нет. Нажмите «Подключить телефон» слева.</div>{/if}
    </div>
    <div class="card" style="margin-top:14px"><h3>На телефон</h3>
      <div class="small muted" style="margin-bottom:10px">Файлы для телефона: положите их сюда или в папку <a href="#open" data-open="phone-outbox">«{PH?.outbox || ''}»</a> — телефон увидит их на вкладке «Получено».</div>
      <div class="row" style="margin-bottom:10px"><label class="f grow" style="margin:0">Кому<select bind:value={to}>
        {#if PH && PH.devices.length}{#each PH.devices as d (d.id)}<option value={d.id}>{d.name}</option>{/each}{:else}<option value="">— сначала подключите телефон —</option>{/if}
      </select></label></div>
      <div class="drop" class:over role="button" tabindex="0"
        ondragover={e => { e.preventDefault(); over = true; }} ondragleave={() => (over = false)}
        ondrop={e => { e.preventDefault(); over = false; send([...(e.dataTransfer?.files || [])]); }}
        onclick={async () => send(await pickFiles({ multiple: true }))} onkeydown={() => {}}>Перетащите файлы сюда или <a href="#pick" onclick={e => e.preventDefault()}>выберите</a></div>
      <div class="small" style="margin-top:8px">{#each sent as s}<div>{s}</div>{/each}</div>
    </div>
  </div>
</div>
