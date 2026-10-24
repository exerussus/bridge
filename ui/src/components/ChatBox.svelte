<script lang="ts">
  // Лента беседы с полем ввода: файлы (кнопкой, перетаскиванием, вставкой), эмодзи и стикеры, анимация новых.
  import { aurl, api } from '../lib/api';
  import { toast, titleOf, SET, setSet } from '../lib/state.svelte';
  import { esc, size, pickFiles } from '../lib/util';
  import { CHT, MSGS, chMsgs, chAttach, chViewing, markRead, isEmojiOnly, b64, STK, stkLoad, stkPrepare, EMOJI_SETS } from '../lib/chat.svelte';
  import { animMsg, stick } from '../lib/anim';
  import { openLightbox } from '../lib/lightbox.svelte';

  let { conv, placeholder = 'Сообщение… (Enter — отправить)', id = undefined }: { conv: string; placeholder?: string; id?: string } = $props();

  let ta = $state<HTMLTextAreaElement>();
  let feed = $state<HTMLElement>();
  let over = $state(false);
  let pickOn = $state(false);
  let busy = $state(false);
  let text = $state('');
  let lastConv = '';
  $effect(() => { if (conv !== lastConv) { lastConv = conv; text = CHT.drafts[conv] || ''; pickOn = false; } });

  const list = $derived((void MSGS.size, [...MSGS.values()], chMsgs(conv)));
  const att = $derived(CHT.att[conv] || []);
  const unread = $derived((CHT.list.find(c => c.conv === conv) || {}).unread || 0);
  $effect(() => { if (unread && chViewing(conv)) markRead(conv); });

  // ссылки — кликабельны; строки «> …» — цитата
  const linkify = (t: string) => esc(t).replace(/(https?:\/\/[^\s<]+)/g, '<a href="$1" target="_blank" rel="noopener">$1</a>').replace(/^&gt; ?(.*)(\n|$)/gm, '<span class="q">$1</span>');
  const reply = $derived(CHT.reply[conv] || null);
  const whoOf = (m: any) => (m.mine ? 'Вы' : titleOf(m.from)) + (m.author ? ' · ' + m.author : '');
  /** Выделенный внутри сообщения текст (для цитаты и копирования). */
  function selIn(el: Element | null) {
    const sel = getSelection(); if (!sel || sel.isCollapsed || !el) return '';
    return el.contains(sel.anchorNode) && el.contains(sel.focusNode) ? sel.toString().trim() : '';
  }
  function startReply(m: any, el: Element | null) {
    const q = selIn(el?.querySelector('.tx') || null);
    const fs = m.files || [];
    const base = String(m.text || '').trim() || (fs.length ? (fs.every((f: any) => f.sticker) ? 'стикер' : '📎 ' + fs.map((f: any) => f.name).join(', ')) : '');
    CHT.reply[conv] = { id: m.id, from: m.mine ? '' : m.from, author: m.author || '', who: whoOf(m), text: (q || base).slice(0, 300), q: !!q };
    requestAnimationFrame(() => ta?.focus());
  }
  function copyMsg(m: any, el: Element | null) {
    const t = selIn(el?.querySelector('.tx') || null) || String(m.text || '');
    if (!t) { toast('В сообщении нет текста', 'bad'); return; }
    navigator.clipboard?.writeText(t).then(() => toast('Текст скопирован', 'ok'), () => toast('Не удалось скопировать', 'bad'));
  }
  /** Перейти к сообщению, на которое ответили (подсветить). */
  function jump(id: string) {
    const el = feed?.querySelector('[data-mid="' + CSS.escape(id) + '"]') as HTMLElement | null;
    if (!el) { toast('Этого сообщения уже нет в ленте (переписка хранится сутки)'); return; }
    el.scrollIntoView({ block: 'center', behavior: 'smooth' });
    el.classList.remove('hl'); void el.offsetWidth; el.classList.add('hl');
  }
  const fileUrl = (m: any, f: any, dl = false) => aurl('chat/file?id=' + encodeURIComponent(m.id) + '&name=' + encodeURIComponent(f.name) + (dl ? '&dl=1' : ''));
  const time = (t: string) => new Date(t).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
  function sameAs(m: any, prev: any) {
    return prev && prev.from === m.from && prev.author === m.author && !!prev.mine === !!m.mine && +new Date(m.time) - +new Date(prev.time) < 300000;
  }

  function autosize() { if (!ta) return; ta.style.height = 'auto'; ta.style.height = Math.min(160, ta.scrollHeight + 2) + 'px'; }
  async function send() {
    const t = text, files = CHT.att[conv] || [];
    if (!t.trim() && !files.length) return;
    busy = true;
    try {
      const fl: any[] = [];
      for (const f of files) fl.push({ name: f.name, mime: f.type || '', data: b64(await f.arrayBuffer()) });
      const rp = CHT.reply[conv];
      const r = await api('POST', 'chat/send', { conv, text: t, files: fl, reply: rp ? { id: rp.id, from: rp.from, author: rp.author, text: rp.text, q: rp.q } : undefined });
      text = ''; CHT.drafts[conv] = ''; CHT.att[conv] = []; delete CHT.reply[conv]; pickOn = false;
      requestAnimationFrame(autosize);
      if (r.message) MSGS.set(r.message.id, r.message);
      (feed as any)?._stickNow?.();
    } catch (e: any) { toast(e.message, 'bad'); }
    busy = false;
  }
  function onPaste(e: ClipboardEvent) {
    const f = [...(e.clipboardData?.files || [])];
    if (!f.length) return;
    e.preventDefault();
    chAttach(conv, f.map((x, i) => (x.name && x.name !== 'image.png' ? x : new File([x], 'картинка-' + new Date().toISOString().slice(11, 19).replace(/:/g, '') + (i ? '-' + i : '') + '.' + (x.type.split('/')[1] || 'png').replace('jpeg', 'jpg'), { type: x.type }))));
  }
  function objUrl(node: HTMLImageElement, f: File) {
    const u = URL.createObjectURL(f); node.src = u;
    return { destroy() { URL.revokeObjectURL(u); } };
  }

  // ── эмодзи и стикеры ──
  function emoRecent(): string[] { return SET.emo || []; }
  function insertEmoji(e: string) {
    const el = ta!, s0 = el.selectionStart ?? text.length, s1 = el.selectionEnd ?? s0;
    text = text.slice(0, s0) + e + text.slice(s1);
    CHT.drafts[conv] = text;
    requestAnimationFrame(() => { el.focus(); el.selectionStart = el.selectionEnd = s0 + e.length; });
    setSet('emo', [e, ...emoRecent().filter(x => x !== e)].slice(0, 24));
  }
  async function togglePick() { pickOn = !pickOn; if (pickOn && STK.tab !== 'emo' && !STK.list) await stkLoad(); }
  async function ptab(t: string) { STK.tab = t; if (t !== 'emo') { STK.list = null; await stkLoad(); } }
  async function sendSticker(id: string, ev: Event) {
    const img = ev.currentTarget as HTMLElement; img.style.opacity = '.4';
    try { const r = await api('POST', 'chat/send', { conv, text: '', sticker: id }); if (r.message) MSGS.set(r.message.id, r.message); pickOn = false; (feed as any)?._stickNow?.(); } catch (e: any) { toast(e.message, 'bad'); }
    img.style.opacity = '';
  }
  async function stkRemove(id: string, set: string) { try { await api('POST', 'stickers/remove', { id, set }); STK.list = null; await stkLoad(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function stkKeep(id: string) { try { await api('POST', 'stickers/keep', { id }); toast('Добавлен в «Мои»', 'ok'); STK.list = null; await stkLoad(); } catch (e: any) { toast(e.message, 'bad'); } }
  async function stkAdd() {
    for (const f of await pickFiles({ multiple: true, accept: 'image/*' })) {
      try { await api('POST', 'stickers', await stkPrepare(f), true); } catch (e: any) { toast('«' + f.name + '»: ' + e.message, 'bad'); }
    }
    STK.list = null; await stkLoad();
  }
  const stkUrl = (id: string, set: string) => aurl('sticker?id=' + encodeURIComponent(id) + '&set=' + set);
  addEventListener('keydown', e => { if (e.key === 'Escape') pickOn = false; });
</script>

<div class="chatbox" {id} class:over role="region"
  ondragover={e => { if (e.dataTransfer?.types.includes('Files')) { e.preventDefault(); over = true; } }}
  ondragleave={e => { if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) over = false; }}
  ondrop={e => { if (!e.dataTransfer?.files.length) return; e.preventDefault(); over = false; chAttach(conv, [...e.dataTransfer.files]); }}>
  <div class="feed noglyph" bind:this={feed} use:stick>
    {#each list as m, i (m.id)}
      {@const prev = list[i - 1]}
      {@const fs = m.files || []}
      {@const stk = fs.length && fs.every((f: any) => f.sticker) && !String(m.text || '').trim()}
      {@const big = !fs.length && isEmojiOnly(m.text)}
      {@const pics = fs.filter((f: any) => !f.sticker && /^image\//.test(f.mime || ''))}
      {@const other = fs.filter((f: any) => !/^image\//.test(f.mime || ''))}
      {#if !sameAs(m, prev) && !m.mine}<div class="who">{titleOf(m.from)}{m.author ? ' · ' + m.author : ''}</div>{/if}
      <div class="msg {m.mine ? 'out' : 'in'}" class:stk class:bigemo={big} data-mid={m.id} use:animMsg={{ id: m.id, conv }}>
        {#if m.mine && m.author}<span class="meta" style="margin:0 0 4px">{m.author}</span>{/if}
        {#if m.reply && m.reply.id}<button class="rq" class:quote={m.reply.q} title="Перейти к сообщению" onclick={() => jump(m.reply.id)}><b>{m.reply.from ? titleOf(m.reply.from) : 'Вы'}{m.reply.author ? ' · ' + m.reply.author : ''}</b><span>{m.reply.text}</span></button>{/if}
        <span class="tx">{@html linkify(m.text || '')}</span>
        <span class="mact" role="toolbar" aria-label="Действия с сообщением">
          <button title="Ответить (выделите часть текста — уйдёт цитатой)" onmousedown={e => e.preventDefault()} onclick={e => startReply(m, (e.currentTarget as HTMLElement).closest('.msg'))}>↩</button>
          <button title="Копировать текст (или выделенное)" disabled={!m.text} onmousedown={e => e.preventDefault()} onclick={e => copyMsg(m, (e.currentTarget as HTMLElement).closest('.msg'))}>⧉</button>
        </span>
        {#each fs.filter((f: any) => f.sticker) as f}
          <img class="stkimg" loading="lazy" alt="стикер" src={fileUrl(m, f)} draggable="true"
            onclick={() => openLightbox({ src: fileUrl(m, f), dl: fileUrl(m, f, true), name: f.name, sticker: m.mine ? '' : f.name })} role="presentation" />
        {/each}
        {#if pics.length}
          <div class="pics">{#each pics as f}<img loading="lazy" alt={f.name} src={fileUrl(m, f)} draggable="true" onclick={() => openLightbox({ src: fileUrl(m, f), dl: fileUrl(m, f, true), name: f.name, sticker: '' })} role="presentation" />{/each}</div>
        {/if}
        {#if other.length}
          <div class="files">{#each other as f}<a class="fch" href={fileUrl(m, f, true)} download={f.name}>📁 {f.name} <span class="muted">{size(f.size)}</span></a>{/each}</div>
        {/if}
        <span class="meta">{time(m.time)}{m.mine ? (m.status === 'queued' ? ' · ждёт: адресат не на связи' : m.status === 'pending' ? ' · отправляю…' : '') : ''}</span>
      </div>
    {:else}
      <div class="empty">Сообщений нет. Переписка хранится сутки.</div>
    {/each}
  </div>
  <div class="rbar" class:hide={!reply}>
    {#if reply}<span class="ricon">{reply.q ? '❝' : '↩'}</span><span class="rbody"><b>{reply.q ? 'Цитата: ' : 'Ответ: '}{reply.who}</b><span>{reply.text}</span></span><button class="b sm ib" title="Не отвечать (Esc)" onclick={() => delete CHT.reply[conv]}>✕</button>{/if}
  </div>
  <div class="att">
    {#each att as f, i}
      <span class="fch">{#if /^image\//.test(f.type)}<img use:objUrl={f} alt="" />{:else}📁{/if} {f.name} <span class="muted">{size(f.size)}</span><button title="Убрать" onclick={() => att.splice(i, 1)}>✕</button></span>
    {/each}
  </div>
  <div class="pick noglyph" class:on={pickOn}>
    {#if pickOn}
      <div class="ptabs">
        {#each [['emo', 'Эмодзи'], ['mine', 'Мои стикеры'], ['got', 'Присланные']] as [k, t]}<button class:on={STK.tab === k} onclick={() => ptab(k)}>{t}</button>{/each}
        <div class="grow"></div><button title="Закрыть" onclick={() => (pickOn = false)}>✕</button>
      </div>
      <div class="pbody">
        {#if STK.tab === 'emo'}
          {#if emoRecent().length}<div class="pgrp">Недавние</div><div class="emog">{#each emoRecent() as e}<button onclick={() => insertEmoji(e)}>{e}</button>{/each}</div>{/if}
          {#each Object.entries(EMOJI_SETS) as [g, l]}<div class="pgrp">{g}</div><div class="emog">{#each l.split(' ') as e}<button onclick={() => insertEmoji(e)}>{e}</button>{/each}</div>{/each}
        {:else if !STK.list}
          <div class="small muted" style="padding:6px 2px">Загружаю…</div>
        {:else if STK.tab === 'mine'}
          <div class="stkg"><button class="stkadd" title="Картинки и гифки до 2 МБ — хранятся у вас" onclick={stkAdd}>＋<span>Добавить</span></button>
            {#each STK.list.mine || [] as x (x.id)}<span class="stk"><img loading="lazy" src={stkUrl(x.id, 'mine')} title="Отправить" alt="" onclick={e => sendSticker(x.id, e)} role="presentation" /><button class="x" title="Удалить из моих" onclick={() => stkRemove(x.id, 'mine')}>✕</button></span>{/each}</div>
          {#if !(STK.list.mine || []).length}<div class="small muted" style="padding:6px 2px">Добавьте картинки или гифки — они хранятся на этом ПК. Отправленный стикер у получателя сохранится в «Присланные».</div>{/if}
        {:else}
          {#if (STK.list.got || []).length}
            <div class="stkg">{#each STK.list.got as x (x.id)}<span class="stk"><img loading="lazy" src={stkUrl(x.id, 'got')} alt="" title={'Отправить · прислал ' + titleOf(x.from || '')} onclick={e => sendSticker(x.id, e)} role="presentation" /><button class="k" title="В мои" onclick={() => stkKeep(x.id)}>★</button><button class="x" title="Удалить" onclick={() => stkRemove(x.id, 'got')}>✕</button></span>{/each}</div>
          {:else}<div class="small muted" style="padding:6px 2px">Здесь появятся стикеры, которые вам присылают. ★ — забрать в свои.</div>{/if}
        {/if}
      </div>
    {/if}
  </div>
  <div class="comp">
    <button class="b" title="Приложить файлы (или перетащите, или вставьте картинку Ctrl+V)" onclick={async () => chAttach(conv, await pickFiles({ multiple: true }))}>📎</button>
    <button class="b" title="Эмодзи и стикеры" onclick={togglePick}>😊</button>
    <textarea bind:this={ta} rows="1" {placeholder} bind:value={text}
      oninput={() => { CHT.drafts[conv] = text; autosize(); }} onpaste={onPaste}
      onkeydown={e => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } else if (e.key === 'Escape' && CHT.reply[conv]) { e.preventDefault(); e.stopPropagation(); delete CHT.reply[conv]; } }}></textarea>
    <button class="b pri" data-send disabled={busy} onclick={send}>Отправить</button>
  </div>
</div>
