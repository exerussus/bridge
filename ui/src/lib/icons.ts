// Тонкие линейные значки (цвет — как у текста). В шаблонах — <Gi n="folder" />,
// в готовом HTML (справка, markdown) эмодзи заменяет действие glyphs.

export const GI: Record<string, string> = {
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  trash: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/>',
  pulse: '<path d="M3 12h4l2-5 4 10 2-5h6"/>',
  mail: '<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>',
  sync: '<path d="M20 11a8 8 0 0 0-14.5-4.5L4 8M4 4v4h4M4 13a8 8 0 0 0 14.5 4.5L20 16M20 20v-4h-4"/>',
  quiet: '<path d="M6 16V11a6 6 0 0 1 9.5-4.9M18 11v5l1.5 2H9M10 20a2 2 0 0 0 4 0M4 4l16 16"/>',
  bell: '<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20a2 2 0 0 0 4 0"/>',
  theme: '<circle cx="12" cy="12" r="8"/><path d="M12 4a8 8 0 0 0 0 16z" fill="currentColor" stroke="none"/>',
  pause: '<path d="M9 6v12M15 6v12"/>', play: '<path d="M8 5l11 7-11 7z"/>', stop: '<rect x="6.5" y="6.5" width="11" height="11" rx="1.5"/>',
  phone: '<path d="M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a1 1 0 0 1-1 1A16 16 0 0 1 4 5a1 1 0 0 1 1-1z"/>',
  screen: '<rect x="3" y="4" width="18" height="12" rx="1.5"/><path d="M8 20h8M12 16v4"/>',
  eye: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>',
  hand: '<path d="M8 13V6.5a1.5 1.5 0 0 1 3 0V12M11 11V5a1.5 1.5 0 0 1 3 0v6M14 11V6.5a1.5 1.5 0 0 1 3 0V14a6 6 0 0 1-6 6h-.5A5.5 5.5 0 0 1 5 15.5V12a1.5 1.5 0 0 1 3 0"/>',
  mic: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21"/>',
  micoff: '<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M4 4l16 16"/>',
  head: '<path d="M4 15v-3a8 8 0 0 1 16 0v3"/><rect x="3" y="14" width="4" height="6" rx="1.5"/><rect x="17" y="14" width="4" height="6" rx="1.5"/>',
  mute: '<path d="M4 9h4l5-4v14l-5-4H4zM17 9l4 6M21 9l-4 6"/>',
  sound: '<path d="M4 9h4l5-4v14l-5-4H4zM16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12"/>',
  pop: '<rect x="3" y="8" width="13" height="13" rx="1.5"/><path d="M8 8V4h13v13h-4"/>',
  expand: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  layers: '<path d="M12 3l9 5-9 5-9-5z"/><path d="M3 13l9 5 9-5"/>',
  list: '<rect x="5" y="3.5" width="14" height="17" rx="2"/><path d="M9 8h6M9 12h6M9 16h4"/>',
  block: '<circle cx="12" cy="12" r="8.5"/><path d="M6 6l12 12"/>',
  users: '<circle cx="9" cy="8" r="3"/><path d="M3.5 19c.8-3.2 3-5 5.5-5s4.7 1.8 5.5 5"/><circle cx="17" cy="9" r="2.3"/><path d="M15.8 13.6c2.2.2 3.9 1.8 4.6 4.4"/>',
  mobile: '<rect x="7" y="3" width="10" height="18" rx="2"/><path d="M11 18h2"/>',
  chat: '<path d="M4 6h16v10H9l-5 4z"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1"/>',
  send: '<path d="M4 12l16-7-6 16-3-7z"/>', up: '<path d="M12 19V5M6 11l6-6 6 6"/>', down: '<path d="M12 5v14M6 13l6 6 6-6"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  railL: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>',
  railR: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/>',
  call: '<path d="M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2"/>',
  people: '<circle cx="9" cy="8" r="3"/><path d="M3.5 19c.8-3.2 3-5 5.5-5s4.7 1.8 5.5 5"/><circle cx="17" cy="9" r="2.3"/>',
  board: '<path d="M4 20l4-1 10-10-3-3L5 16z"/><path d="M14 6l3 3"/>',
  invite: '<circle cx="10" cy="8" r="3"/><path d="M4 19c.8-3.2 3-5 6-5 1.5 0 2.8.4 3.8 1.2M18 13v6M15 16h6"/>',
  shareUp: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4M12 7v6M9 10l3-3 3 3"/>',
  ptt: '<rect x="7" y="3" width="10" height="18" rx="3"/><path d="M10 7h4"/>',
  leave: '<path d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3"/><path d="M10 16l-4-4 4-4M6 12h10"/>',
  chev: '<path d="M6 9l6 6 6-6"/>',
  mic2: '<path d="M12 3a3 3 0 0 1 3 3v6a3 3 0 0 1-6 0V6a3 3 0 0 1 3-3z"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>',
  x: '<path d="M6 6l12 12M18 6L6 18"/>',
  check: '<path d="M5 12l5 5 9-10"/>',
  edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/>',
  lock: '<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
};

/** Значки вкладок слева (у активной — свечение). */
export const TAB_ICONS: Record<string, string> = {
  home: '<path d="M4 11l8-6 8 6v8a1 1 0 0 1-1 1h-4v-6H9v6H5a1 1 0 0 1-1-1z"/>',
  peers: GI.users,
  send: '<path d="M4 12l16-7-6 16-3-7z"/><path d="M11 14l9-9"/>',
  folders: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M8 13h8"/>',
  shares: GI.layers,
  records: GI.list,
  voice: '<path d="M4 10v4M8 7v10M12 4v16M16 8v8M20 11v2"/>',
  agent: '<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M18.5 15.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z"/>',
  phone: '<rect x="6.5" y="2.5" width="11" height="19" rx="2.5"/><path d="M10.5 18.5h3"/>',
  chat: '<path d="M4 6h16v10H9l-5 4z"/><path d="M8 10h8M8 13h5"/>',
  assistant: '<path d="M5 5h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-7l-4 3v-3H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z"/><path d="M8 11h.01M12 11h.01M16 11h.01"/>',
  settings: '<circle cx="12" cy="12" r="3"/><path d="M12 2.5v3M12 18.5v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2.5 12h3M18.5 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1"/>',
  access: '<circle cx="8" cy="14" r="4"/><path d="M11 11l8-8M16 6l2 2M14 8l2 2"/>',
  log: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><path d="M7 9l3 3-3 3M12 15h5"/>',
  help: '<circle cx="12" cy="12" r="8.5"/><path d="M9.6 9.5a2.5 2.5 0 1 1 3.4 2.3c-.6.3-1 .8-1 1.5v.5"/><path d="M12 17h.01"/>',
};

const EMO: Record<string, string> = {
  '📂': 'folder', '📁': 'folder', '🗑': 'trash', '🩺': 'pulse', '✉': 'mail', '⟳': 'sync', '🔔': 'bell', '🌓': 'theme', '⏸': 'pause', '▶': 'play', '⏹': 'stop',
  '📞': 'phone', '🖥': 'screen', '👁': 'eye', '🙋': 'hand', '🎙': 'mic', '🎧': 'head', '🔕': 'micoff', '🔇': 'mute', '🔊': 'sound', '⧉': 'pop', '⛶': 'expand',
  '💬': 'chat', '⚙': 'gear', '🗂': 'layers', '📋': 'list', '🚫': 'block', '👥': 'users', '📤': 'send', '⬆': 'up', '⬇': 'down', '📱': 'mobile',
};
const EMO_SRC = '(' + Object.keys(EMO).join('|') + ')️?';
const EMO_TEST = new RegExp(EMO_SRC);

export const svg = (name: string) => '<svg class="gi" viewBox="0 0 24 24" aria-hidden="true">' + (GI[name] || '') + '</svg>';

/** Заменить эмодзи в тексте внутри root на линейные значки (только для готового HTML, не для реактивных узлов). */
export function glyphs(root: Element | null) {
  if (!root) return;
  const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const list: Text[] = [];
  while (w.nextNode()) if (EMO_TEST.test((w.currentNode as Text).data)) list.push(w.currentNode as Text);
  for (const n of list) {
    const p = n.parentNode as Element | null;
    if (!p || /^(OPTION|TITLE|SCRIPT|STYLE|TEXTAREA)$/.test(p.nodeName) || p.closest?.('.noglyph')) continue;
    const re = new RegExp(EMO_SRC, 'g'), frag = document.createDocumentFragment();
    let last = 0, m: RegExpExecArray | null;
    while ((m = re.exec(n.data))) {
      frag.append(n.data.slice(last, m.index));
      const t = document.createElement('span');
      t.innerHTML = svg(EMO[m[1]]);
      frag.append(t.firstChild!);
      last = m.index + m[0].length;
    }
    frag.append(n.data.slice(last));
    p.replaceChild(frag, n);
  }
}

/** Действие Svelte: значки в статичном HTML (use:glyphAction). */
export function glyphAction(node: Element) {
  glyphs(node);
  return {};
}
