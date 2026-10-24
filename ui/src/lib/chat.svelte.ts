// Чаты: личные и каналов (хранятся сутки). Долгий опрос chat/messages держит ленту свежей.
import { SvelteMap } from 'svelte/reactivity';
import { api } from './api';
import { ui, toast, titleOf, badgeCount, SET } from './state.svelte';
import { notifyTo } from './notify';
import { msgBeep } from './audio';

export const CHT = $state({
  n: 0,
  list: [] as any[],
  cur: null as string | null,
  first: true,
  drafts: {} as Record<string, string>,
  att: {} as Record<string, File[]>,
  /** ответ, который сейчас пишется: беседа → {id, from, author, text, q} */
  reply: {} as Record<string, any>,
  /** Канал, открытый в «Каналах» (его чат справа считается «виден»). */
  roomConv: '' as string,
});
export const MSGS = new SvelteMap<string, any>();

export function chMsgs(conv: string) {
  return [...MSGS.values()].filter(m => m.conv === conv).sort((a, b) => +new Date(a.time) - +new Date(b.time) || a.n - b.n);
}

export async function chLoadList() {
  try {
    const r = await api('GET', 'chat');
    CHT.list = r.conversations;
    const cur = ui.tab === 'chat' && CHT.cur ? (CHT.list.find(c => c.conv === CHT.cur) || {}).unread || 0 : 0;
    badgeCount('chat', Math.max(0, r.unread - cur));
  } catch {}
}

export function chViewing(conv: string) {
  return !document.hidden && ((ui.tab === 'chat' && CHT.cur === conv) || (ui.tab === 'voice' && !!CHT.roomConv && conv === CHT.roomConv));
}

export function markRead(conv: string) {
  api('POST', 'chat/read?conv=' + encodeURIComponent(conv)).then(chLoadList).catch(() => {});
}

function chNotify(m: any) {
  if (chViewing(m.conv)) { markRead(m.conv); return; }
  const who = titleOf(m.from) + (m.author ? ' · ' + m.author : ''), dm = m.conv.startsWith('dm:');
  const where = dm ? '' : ' в «' + ((CHT.list.find(c => c.conv === m.conv) || {}).title || 'канале') + '»';
  const files = m.files || [];
  const txt = m.text || (files.length ? (files.every((f: any) => f.sticker) ? 'стикер' : '📎 ' + files[0].name) : '');
  toast('💬 ' + who + where + ': ' + txt.slice(0, 100));
  if (dm && SET.chatSound) msgBeep();
  if (document.hidden) notifyTo('Мост · ' + who + where, txt, 'chat');
}

let started = false;
export async function chStart() {
  if (started) return;
  started = true;
  await chLoadList();
  for (;;) {
    try {
      const r = await api('GET', 'chat/messages?after=' + CHT.n + '&timeout=' + (CHT.first ? 0 : 25));
      const fresh: any[] = [];
      for (const m of r.items) {
        if (!MSGS.has(m.id) && !m.mine && !CHT.first) fresh.push(m);
        MSGS.set(m.id, m);
        CHT.n = Math.max(CHT.n, m.n);
      }
      CHT.first = false;
      if (r.items.length) await chLoadList();
      for (const m of fresh) chNotify(m);
    } catch { await new Promise(r => setTimeout(r, 5000)); }
  }
}

export const b64 = (buf: ArrayBuffer) => {
  let s = '';
  const b = new Uint8Array(buf);
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode.apply(null, b.subarray(i, i + 0x8000) as any);
  return btoa(s);
};

export function chAttach(conv: string, files: File[]) {
  const list = (CHT.att[conv] = CHT.att[conv] || []);
  for (const f of files) {
    if (f.size > 10 * 1048576) { toast('«' + f.name + '» больше 10 МБ', 'bad'); continue; }
    list.push(f);
  }
}

/** Сообщение только из эмодзи — показать крупно. */
export const isEmojiOnly = (t: string) => {
  t = String(t || '').trim();
  if (!t || t.length > 24) return false;
  return /^(?:\p{Extended_Pictographic}|\p{Emoji_Component}|‍|️|\s)+$/u.test(t) && /\p{Extended_Pictographic}/u.test(t);
};

// ── стикеры ──
export const STK = $state({ list: null as any, tab: 'emo' });
export async function stkLoad() {
  try { STK.list = await api('GET', 'stickers'); } catch (e: any) { STK.list = { mine: [], got: [] }; toast(e.message, 'bad'); }
}
/** Гифка — как есть (с анимацией); большая картинка — ужать до 512 px. */
export async function stkPrepare(f: File): Promise<Blob> {
  if (f.type === 'image/gif' || /\.gif$/i.test(f.name)) { if (f.size > 2 * 1048576) throw new Error('гифка больше 2 МБ'); return f; }
  if (f.size <= 600 * 1024) return f;
  const img = await createImageBitmap(f), k = Math.min(1, 512 / Math.max(img.width, img.height)), c = document.createElement('canvas');
  c.width = Math.round(img.width * k); c.height = Math.round(img.height * k);
  c.getContext('2d')!.drawImage(img, 0, 0, c.width, c.height);
  return await new Promise<Blob>(r => c.toBlob(b => r(b!), 'image/webp', 0.9));
}

export const EMOJI_SETS: Record<string, string> = {
  'Лица': '😀 😃 😄 😁 😆 😅 🤣 😂 🙂 😉 😊 😇 🥰 😍 🤩 😘 😋 😛 😜 🤪 😝 🤗 🤭 🤫 🤔 🤐 🤨 😐 😑 😶 😏 😒 🙄 😬 😌 😔 😪 😴 😷 🤒 🤕 🤢 🤮 🥵 🥶 🥴 😵 🤯 🤠 🥳 😎 🤓 🧐 😕 😟 🙁 😮 😯 😲 😳 🥺 😦 😧 😨 😰 😥 😢 😭 😱 😖 😣 😞 😓 😩 😫 🥱 😤 😡 😠 🤬 😈 👿 💀 💩 🤡 👻 👽 🤖 😺 😸 😹 😻 😼 😽 🙀 😿 😾',
  'Жесты': '👍 👎 👌 🤌 🤏 ✌️ 🤞 🤟 🤘 🤙 👈 👉 👆 👇 ☝️ ✋ 🤚 🖐️ 🖖 👋 🤝 🙏 👏 🙌 👐 🤲 💪 🦾 ✍️ 💅 🤳 👀 👁️ 🧠 🫶 🫡 🫠 🫣 🫢',
  'Сердца': '❤️ 🧡 💛 💚 💙 💜 🖤 🤍 🤎 💔 ❣️ 💕 💞 💓 💗 💖 💘 💝 💟 💯 💢 💥 💫 💦 💨 🔥 ✨ ⭐ 🌟 ⚡ 🎉 🎊 🎁 🏆 🥇',
  'Разное': '☕ 🍵 🍺 🍻 🍷 🥂 🍕 🍔 🍟 🌭 🍿 🍩 🍪 🎂 🍰 🍫 🍎 🍌 🍓 🍉 🥑 🐶 🐱 🐭 🐹 🐰 🦊 🐻 🐼 🐨 🐯 🦁 🐮 🐷 🐸 🐵 🐔 🐧 🐦 🦄 🐝 🦋 🐢 🐍 🐙 🐬 🐳 🌈 ☀️ 🌙 ⛄ 🌧️ 🌸 🌹 🌻 🌲 🍀 🎮 🕹️ 🎧 🎵 🎬 📷 💻 🖥️ 📱 ⌨️ 🖱️ 💾 📎 📌 ✅ ❌ ⚠️ ❓ ❗ 💤 🚀 ✈️ 🚗 🏠 🎯 🎲 ♟️ 🧩',
};
