<script lang="ts">
  // Ключ пользователя (3.7): приложения видят только свой ключ для издателя; здесь — резервная копия, восстановление и смена.
  import { toast } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { pickFiles } from '../lib/util';
  let st = $state<any>(null);
  let pw1 = $state(''), pw2 = $state(''), pwIn = $state('');
  let file = $state<any>(null), fileName = $state('');
  let busy = $state('');
  async function load() { try { st = await api('GET', 'identity'); } catch (e: any) { st = { ok: false, error: e.message }; } }
  $effect(() => { load(); });
  async function exportKey() {
    if (pw1.length < 8) { toast('Пароль — не короче 8 знаков', 'bad'); return; }
    if (pw1 !== pw2) { toast('Пароли не совпадают', 'bad'); return; }
    busy = 'export';
    try {
      const f = await api('POST', 'identity/export', { password: pw1 });
      const b = new Blob([JSON.stringify(f, null, 2)], { type: 'application/json' });
      const u = URL.createObjectURL(b), l = document.createElement('a');
      l.href = u; l.download = 'ключ-моста-' + (st?.fingerprint || '').slice(0, 9) + '.most-key'; document.body.appendChild(l); l.click(); l.remove();
      setTimeout(() => URL.revokeObjectURL(u), 10000);
      pw1 = pw2 = '';
      toast('Резервная копия сохранена — храните её и пароль отдельно', 'ok');
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
  async function pick() {
    const [f] = await pickFiles({ accept: '.most-key,application/json' });
    if (!f) return;
    try { file = JSON.parse(await f.text()); fileName = f.name; } catch { toast('Это не резервная копия ключа', 'bad'); file = null; fileName = ''; }
  }
  async function importKey() {
    if (!file) return;
    if (!confirm('Заменить ключ этого ПК ключом из резервной копии (' + (file.fingerprint || '?') + ')? Текущий ключ сохранится, приложения получат подписанный переход на новый.')) return;
    busy = 'import';
    try {
      const r = await api('POST', 'identity/import', { file, password: pwIn });
      toast(r.same ? 'Это тот же ключ — ничего не изменилось' : 'Ключ восстановлен', 'ok');
      file = null; fileName = ''; pwIn = ''; await load();
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
  async function rotate() {
    if (!confirm('Сменить ключ? Используйте, если ключ мог попасть к чужим. Игры увидят новый ключ и подписанный переход со старого; аккаунты в играх, возможно, придётся подтвердить заново (например, почтой).')) return;
    busy = 'rotate';
    try { st = await api('POST', 'identity/rotate'); toast('Ключ сменён', 'ok'); } catch (e: any) { toast(e.message, 'bad'); }
    busy = '';
  }
</script>

<div class="card"><h3>Ваш ключ</h3>
  <div class="small muted" style="margin-bottom:8px">Постоянный ключ Ed25519 — ваша личность для игр и приложений в мосте. Приложения его не видят: каждому издателю мост выводит свой ключ, а подписывает только строки с именем приложения и назначением. Секрет хранится на этом ПК{st?.protected ? ' под защитой Windows (DPAPI)' : ''} и никуда не отправляется.</div>
  <div class="setrow"><span>Отпечаток</span><span class="mono" style="min-width:180px">{st?.ok ? st.fingerprint : st?.error || '…'}</span><span class="small muted">{st?.previous ? 'прежних ключей: ' + st.previous : ''}</span></div>
</div>
<div class="card"><h3>Резервная копия</h3>
  <div class="small muted" style="margin-bottom:8px">Файл с ключом под паролем (Argon2id + XChaCha20-Poly1305) — чтобы перенести ключ на новый ПК или восстановить после переустановки. Без пароля файл бесполезен; пароль мост не хранит.</div>
  <div class="setrow"><span>Пароль</span><span class="row" style="gap:8px"><input type="password" autocomplete="new-password" placeholder="не короче 8 знаков" bind:value={pw1} style="width:180px" /><input type="password" autocomplete="new-password" placeholder="ещё раз" bind:value={pw2} style="width:180px" /></span><span><button class="b sm pri" style="min-width:150px" disabled={!!busy || !pw1} onclick={exportKey}>{busy === 'export' ? 'Шифрую…' : 'Сохранить копию'}</button></span></div>
</div>
<div class="card"><h3>Восстановить или сменить</h3>
  <div class="setrow"><span>Из копии</span><span class="row" style="gap:8px"><button class="b sm" style="min-width:150px" onclick={pick}>{fileName ? fileName.slice(0, 22) : 'Выбрать файл…'}</button><input type="password" autocomplete="current-password" placeholder="пароль копии" bind:value={pwIn} style="width:180px" /></span><span><button class="b sm pri" style="min-width:150px" disabled={!!busy || !file || !pwIn} onclick={importKey}>{busy === 'import' ? 'Проверяю…' : 'Восстановить'}</button></span></div>
  <div class="setrow"><span>Новый ключ</span><span class="small muted">если ключ мог попасть к чужим</span><span><button class="b sm danger" style="min-width:150px" disabled={!!busy} onclick={rotate}>{busy === 'rotate' ? 'Меняю…' : 'Сменить ключ…'}</button></span></div>
  <div class="small muted" style="margin-top:6px">Прежние ключи остаются на этом ПК: ими мост подписывает переход «старый → новый», чтобы игры узнали вас.</div>
</div>
