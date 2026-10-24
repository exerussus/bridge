<script lang="ts">
  import { ui, toast, titleOf } from '../lib/state.svelte';
  import { api, download } from '../lib/api';
  import { esc, size, pickFiles } from '../lib/util';
  import { b64 } from '../lib/chat.svelte';
  import { AS, AS_ITEMS, alarmStop, asRead } from '../lib/assistant.svelte';

  let text = $state('');
  let ta = $state<HTMLTextAreaElement>();
  let feed = $state<HTMLElement>();
  let over = $state(false);
  const list = $derived([...AS_ITEMS.values()].sort((a, b) => (a.first || a.n) - (b.first || b.n)));
  $effect(() => { asRead(); setTimeout(() => ta?.focus(), 50); });
  // держать низ ленты, если читатель внизу
  $effect(() => {
    void list.length; void AS.typing;
    if (!feed) return;
    const atBottom = feed.scrollHeight - feed.scrollTop - feed.clientHeight < 80;
    if (atBottom || AS.stick) requestAnimationFrame(() => { if (feed) feed.scrollTop = feed.scrollHeight; AS.stick = false; });
  });
  const linkify = (t: string) => esc(t).replace(/(https?:\/\/[^\s<]+)/g, '<a href="$1" target="_blank" rel="noopener">$1</a>');
  const time = (t: string) => new Date(t).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  function meta(it: any) {
    let m = time(it.time);
    if (it.dir === 'out') m += it.status === 'pending' ? ' · ожидает отправки' : it.status === 'failed' ? ' · не доставлено: ' + (it.error || '') : !AS.online ? ' · доставлено мосту, ассистент не на связи' : ' · отправлено';
    if (it.kind === 'notice') m = (it.alarm ? '⏰ будильник · ' : '🔔 напоминание · ') + m;
    return m;
  }
  function autosize() { if (!ta) return; ta.style.height = 'auto'; ta.style.height = Math.min(180, ta.scrollHeight + 2) + 'px'; }
  async function sendText() {
    if (!text.trim()) return;
    try { await api('POST', 'assistant/send', { kind: 'text', text }); text = ''; requestAnimationFrame(autosize); AS.stick = true; } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function sendFiles(files: File[]) {
    if (!files.length) return;
    const big = files.find(f => f.size > 10 * 1048576);
    if (big) { toast('«' + big.name + '» больше 10 МБ — ассистенту не отправить', 'bad'); return; }
    const list: any[] = [];
    for (const f of files) list.push({ name: f.name, mime: f.type || 'application/octet-stream', size: f.size, data: b64(await f.arrayBuffer()) });
    try { await api('POST', 'assistant/send', { kind: 'file', text, files: list }); text = ''; AS.stick = true; } catch (e: any) { toast(e.message, 'bad'); }
  }
  async function press(it: any, b: any, ev: Event) {
    if (AS.alarm) alarmStop();
    (ev.currentTarget as HTMLElement).closest('.btns')?.querySelectorAll('button').forEach(x => ((x as HTMLButtonElement).disabled = true));
    try { await api('POST', 'assistant/send', { kind: 'action', action: { name: b.action, args: b.args || {} }, label: b.label, replyTo: it.id }); AS.stick = true; } catch (e: any) { toast(e.message, 'bad'); }
  }

  // голос: WAV PCM16, моно, 16 кГц; стоп — после 1.2 с тишины (когда уже говорили), по кнопке или через 120 с
  let rec = $state({ on: false, sec: 0, lvl: 0 });
  let R: any = {};
  async function recStart() {
    if (rec.on) { recStop(true); return; }
    let stream: MediaStream;
    try { stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true } }); } catch (e: any) { toast('Микрофон: ' + e.message, 'bad'); return; }
    const ac = new AudioContext({ sampleRate: 16000 }), src = ac.createMediaStreamSource(stream), proc = ac.createScriptProcessor(4096, 1, 1);
    R = { stream, ac, src, proc, chunks: [], t0: Date.now(), spoke: false, quiet: 0, rate: ac.sampleRate };
    rec.on = true; rec.sec = 0;
    proc.onaudioprocess = e => {
      if (!rec.on) return;
      const d = e.inputBuffer.getChannelData(0);
      R.chunks.push(new Float32Array(d));
      let sum = 0; for (const x of d) sum += x * x;
      const rms = Math.sqrt(sum / d.length);
      rec.lvl = Math.min(100, rms * 600);
      if (rms > 0.02) { R.spoke = true; R.quiet = 0; } else if (R.spoke) { R.quiet += d.length / R.rate; if (R.quiet > 1.2) recStop(true); }
      const sec = (Date.now() - R.t0) / 1000; rec.sec = Math.floor(sec);
      if (sec >= 120) recStop(true);
    };
    src.connect(proc); proc.connect(ac.destination);
  }
  function wav(chunks: Float32Array[], rate: number) {
    const n = chunks.reduce((a, c) => a + c.length, 0), buf = new ArrayBuffer(44 + n * 2), v = new DataView(buf);
    const w = (o: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
    w(0, 'RIFF'); v.setUint32(4, 36 + n * 2, true); w(8, 'WAVE'); w(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
    v.setUint32(24, rate, true); v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true); w(36, 'data'); v.setUint32(40, n * 2, true);
    let o = 44; for (const c of chunks) for (const x of c) { const s = Math.max(-1, Math.min(1, x)); v.setInt16(o, s < 0 ? s * 0x8000 : s * 0x7fff, true); o += 2; }
    return buf;
  }
  async function recStop(send: boolean) {
    if (!rec.on) return;
    rec.on = false;
    try { R.proc.disconnect(); R.src.disconnect(); } catch {}
    R.stream.getTracks().forEach((t: MediaStreamTrack) => t.stop());
    try { await R.ac.close(); } catch {}
    if (!send) return;
    const seconds = Math.round(R.chunks.reduce((a: number, c: Float32Array) => a + c.length, 0) / R.rate);
    if (seconds < 1) { toast('Слишком коротко'); return; }
    const data = b64(wav(R.chunks, R.rate));
    try { await api('POST', 'assistant/send', { kind: 'voice', text, voice: { mime: 'audio/wav', data }, seconds }); text = ''; AS.stick = true; } catch (e: any) { toast(e.message, 'bad'); }
  }
</script>

<div class="card chat" id="asDrop" class:over role="region"
  ondragover={e => { if (e.dataTransfer?.types.includes('Files')) { e.preventDefault(); over = true; } }}
  ondragleave={e => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) over = false; }}
  ondrop={e => { e.preventDefault(); over = false; sendFiles([...(e.dataTransfer?.files || [])]); }}>
  <div class="row" style="margin-bottom:6px">
    <span class="pill {AS.online ? 'ok' : AS.host ? 'warn' : ''}">{!AS.host ? 'ассистент не подключён' : AS.title + (AS.online ? ' на связи' : ' не на связи')}</span>
    <span class="small muted grow">{!AS.host ? '' : AS.isHost ? 'работает на этом ПК' : 'работает на ПК «' + titleOf(AS.host) + '»' + (AS.online ? '' : ' — сообщения уйдут, когда он вернётся')}</span>
    <button class="b sm" class:hide={!AS.alarm} onclick={alarmStop}>Выключить будильник</button>
  </div>
  <div id="asFeed" bind:this={feed}>
    {#each list as it (it.id)}
      {#if it.dir === 'out' && it.kind === 'action'}
        <div class="msg act">нажато «{it.text}»{it.status === 'pending' ? ' · ожидает отправки' : ''}</div>
      {:else}
        <div class="msg {it.dir}" class:notice={it.kind === 'notice'} class:alarm={it.alarm}>
          {#if it.kind === 'voice'}🎙 голосовое{it.voice && it.voice.seconds ? ', ' + it.voice.seconds + ' с' : ''}{it.text ? '\n' : ''}{/if}{#if it.text}{@html linkify(it.text)}{/if}
          {#if it.files && it.files.length}
            <div class="files">{#each it.files as f}
              {#if f.path}<a class="fch" href="#dl" onclick={e => { e.preventDefault(); download('mailbox/incoming/file?path=' + encodeURIComponent(f.path), f.name).catch(er => toast(er.message, 'bad')); }}>📁 {f.name} <span class="muted">{size(f.size)}</span></a>
              {:else}<span class="fch">📁 {f.name} <span class="muted">{size(f.size)}</span></span>{/if}
            {/each}</div>
          {/if}
          {#if it.buttons && it.buttons.length}
            <div class="btns">
              {#each it.buttons as row}<div class="row">{#each row as b}<button class="b sm" disabled={!!it.pressed} onclick={e => press(it, b, e)}>{b.label}</button>{/each}</div>{/each}
              {#if it.pressed}<span class="meta">нажато «{it.pressed}»</span>{/if}
            </div>
          {/if}
          <span class="meta" class:bad={it.status === 'failed'}>{meta(it)}</span>
        </div>
      {/if}
    {:else}
      {#if AS.host}<div class="empty">Напишите ассистенту — ответ придёт сюда.</div>
      {:else}
        <div class="empty" style="text-align:left;line-height:1.6"><b>Ассистент ещё ни разу не подключался к мосту.</b><br />Вкладка заработает сама, когда ассистент на этом ПК или у соседа обратится к мосту (он проверяет раз в 5 минут).<br />
          Ассистенту нужны: адрес <code>http://127.0.0.1:{location.port || '5390'}</code> и ключ — файл <code>bridge-data\local.token</code> рядом с мостом; в его настройках — <code>[most] chat = true</code>.<br />Проверить вручную: в консоли <code>bridge api GET assistant/info</code> — должен ответить <code>{'{"version":1,…}'}</code>.</div>
      {/if}
    {/each}
    {#if AS.typing}<div class="typing">{AS.title} печатает<i></i><i></i><i></i></div>{/if}
  </div>
  <div id="asRec" class:hide={!rec.on}><span>● запись</span><span>{rec.sec} с</span><span class="lvl"><i style="width:{rec.lvl}%"></i></span><span class="small muted">остановится сама после паузы</span><div class="grow"></div>
    <button class="b sm" onclick={() => recStop(true)}>Готово</button><button class="b sm" onclick={() => recStop(false)}>Отмена</button></div>
  <div id="asComposer" style={AS.host ? '' : 'display:none'}>
    <button class="b" title="Файлы (или перетащите сюда)" onclick={async () => sendFiles(await pickFiles({ multiple: true }))}>📎</button>
    <textarea bind:this={ta} bind:value={text} rows="1" placeholder="Сообщение ассистенту… (Enter — отправить, Shift+Enter — новая строка)" oninput={autosize}
      onkeydown={e => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendText(); } }}></textarea>
    <button class="b" class:pri={rec.on} title="Голосовое сообщение" onclick={recStart}>🎙</button>
    <button class="b pri" onclick={sendText}>Отправить</button>
  </div>
</div>
