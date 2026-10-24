// Общее состояние панели: обзор узла, вкладка, всплывающие окна, уведомления.
import type { Component } from 'svelte';
import { api, app } from './api';

export const TABS = ['home', 'peers', 'send', 'folders', 'shares', 'records', 'voice', 'chat', 'agent', 'assistant', 'phone', 'settings', 'access', 'log', 'help'] as const;
export type Tab = (typeof TABS)[number];

export const TONE: Record<string, string> = {
  home: 'cyan', peers: 'cyan', send: 'green', folders: 'green', shares: 'green', records: 'amber', voice: 'red', chat: 'red', agent: 'amber',
  assistant: 'cyan', phone: 'cyan', settings: 'dim', access: 'amber', log: 'dim', help: 'dim',
};
export const HEAD: Record<string, [string, string]> = {
  home: ['Обзор', 'Что происходит на этом ПК и у соседей.'],
  peers: ['Соседи', 'ПК, с которыми работает мост: связь, адреса, проверка.'],
  send: ['Отправка', 'Переслать файлы соседу: положили — он получил.'],
  folders: ['Общие папки', 'Одна папка на нескольких ПК. У кого доступ — меняет у всех.'],
  shares: ['Пространства', 'Рабочие папки проектов с записями и правилами приёма.'],
  records: ['Записи', 'Задачи, обсуждения и команды между ПК.'],
  voice: ['Каналы', 'Голос и показ экрана. Зашли в канал — вы в разговоре.'],
  agent: ['Агенты', 'Claude на одном ПК поручает задачи Claude на другом — и получает отчёт.'],
  chat: ['Чаты', 'Переписка с соседями и в каналах. Сообщения хранятся сутки.'],
  assistant: ['Ассистент', 'Чат с ассистентом семьи: текст, голос, файлы. Напоминания — уведомлением.'],
  phone: ['Телефон', 'Мост в телефоне: файлы туда и обратно — дома и вне дома.'],
  settings: ['Настройки', 'Микрофон, звук, сигналы и вид — для этого ПК.'],
  access: ['Права и группы', 'Кто что видит. Группы — чтобы не настраивать каждого.'],
  log: ['Журнал', 'Что делал мост и почему.'],
  help: ['Справка', 'Как всё устроено и что делать, если не работает.'],
};

type Toast = { id: number; msg: string; kind: string };
type Modal = { c: Component<any>; p: Record<string, any> } | null;

export const ui = $state({
  tab: 'home' as string,
  ov: null as any,
  conn: '' as string,
  /** Обзор хоть раз пришёл (до этого вкладки ждут). */
  ready: false,
  modal: null as Modal,
  toasts: [] as Toast[],
  /** Записи: выбранное пространство и запись (переход с «Обзора»). */
  recShare: '' as string,
  curRecord: null as string | null,
  /** Счётчики на вкладках, которые считает не обзор (чаты, голос, ассистент, агенты, телефон). */
  counts: {} as Record<string, number>,
  /** Тик раз в секунду — для «… назад» и опросов. */
  now: Date.now(),
  /** Бейдж «ждёт вас» в заголовке окна. */
  attention: 0,
  /** Раздел настроек. */
  stTab: 'audio' as string,
  /** «Агенты»: какую нить открыть (переход из «Записей»). */
  agentThread: '' as string,
  /** Кто заглушил мой микрофон (пусто — никто). */
  forcedBy: '' as string,
});

let toastId = 0;
export function toast(msg: string, kind = '') {
  const t = { id: ++toastId, msg, kind };
  ui.toasts.push(t);
  setTimeout(() => { const i = ui.toasts.findIndex(x => x.id === t.id); if (i >= 0) ui.toasts.splice(i, 1); }, kind === 'bad' ? 9000 : 4000);
}

export function openModal<P extends Record<string, any>>(c: Component<P>, p: P = {} as P) { ui.modal = { c, p }; }
export function closeModal() { ui.modal = null; }

export function titleOf(name: string): string {
  const p = ui.ov?.peers?.find((x: any) => x.name === name);
  return (p && p.title) || name || '';
}
export function peerTitle(name: string): string {
  if (ui.ov && name === ui.ov.node.node) return 'вы (' + name + ')';
  const p = ui.ov?.peers?.find((x: any) => x.name === name);
  return p && p.title ? p.title + ' (' + name + ')' : name;
}
export const me = () => ui.ov?.node?.node || '';

const tabListeners = new Set<(t: string) => void>();
export function onTab(f: (t: string) => void) { tabListeners.add(f); return () => tabListeners.delete(f); }

export function setTab(t: string) {
  if (!TABS.includes(t as Tab)) t = 'home';
  ui.tab = t;
  if (location.hash.slice(1) !== t) history.replaceState(null, '', '#' + t);
  for (const f of tabListeners) f(t);
}

let bootVer = '';
let loading: Promise<void> | null = null;
/** Перечитать обзор узла. Мост обновился и перезапустился — перезагрузить панель целиком. */
export function load(): Promise<void> {
  if (loading) return loading;
  loading = (async () => {
    try {
      const ov = await api('GET', 'overview');
      if (bootVer && ov.node.version !== bootVer) { location.reload(); return; }
      bootVer = bootVer || ov.node.version;
      ui.ov = ov;
      ui.conn = '';
      ui.ready = true;
    } catch {
      ui.conn = 'узел не отвечает';
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/** Заголовок окна: в браузере — вкладки, в приложении — окна в панели задач. */
export function setTitle(t: string) {
  if (document.title !== t) { document.title = t; app('title', { title: t }); }
}

export function badgeCount(name: string, v: number) { ui.counts[name] = v; }

// ── настройки этого ПК (хранит мост: bridge-data/ui.json; переживают переустановку и смену окна) ──
export const SET_DEFAULT: Record<string, any> = {
  mic: '', out: '', micGain: 1, outVol: 1, ns: true, ec: true, agc: true, sfx: true, sfxVol: 0.6, ping: 'drop', pingVol: 0.7, chatSound: true,
  pttKey: 'Space', joinSounds: true, joinVol: 0.7,
  shareQ: 'text', shareAudio: true, shareCursor: true, railFold: false, chatFold: false,
  pingEvery: 3, pingDnd: false, pingFlash: true, bBrush: 12, bTxt: { ff: '', fb: false, fi: false, fu: false, fa: '', fs: 22 },
  bSync: 50, bCursors: true, bGrid: 'dots', bSnap: false, bSmooth: true, bImg: 2560,
  // доска 3.0: фон холста (цвет, пусто — по теме) и яркость сетки
  bBg: '', bBright: 100,
  theme: 'dark', notify: false, sendPeer: '', emo: [] as string[], seenVersion: '',
};
export const SET = $state<Record<string, any>>({ ...SET_DEFAULT });
let setTimer: any = null;
let setLoaded = false;

export async function loadSettings() {
  let saved: any = null;
  try { saved = (await api('GET', 'ui/settings')).settings; } catch {}
  if (!saved || !Object.keys(saved).length) {
    // перенос из браузерной панели (до 3.0 настройки жили в браузере)
    try {
      const old = JSON.parse(localStorage.getItem('bridge-settings') || '{}');
      saved = { ...old };
      const th = localStorage.getItem('bridge-theme'); if (th) saved.theme = th;
      const sv = localStorage.getItem('bridge-seen-version'); if (sv) saved.seenVersion = sv;
      const sp = localStorage.getItem('bridge-send-peer'); if (sp) saved.sendPeer = sp;
      if (localStorage.getItem('bridge-sfx') === '0') saved.sfx = false;
      const emo = localStorage.getItem('bridge-emo'); if (emo) saved.emo = JSON.parse(emo);
    } catch {}
  }
  Object.assign(SET, SET_DEFAULT, saved || {});
  setLoaded = true;
  document.documentElement.dataset.theme = SET.theme || 'dark';
}

/** Сохранить настройку (в мост — с задержкой, пачкой). */
export function setSet(k: string, v: any) {
  SET[k] = v;
  saveSettings();
}
export function saveSettings() {
  if (!setLoaded) return;
  clearTimeout(setTimer);
  setTimer = setTimeout(() => { api('POST', 'ui/settings', { settings: $state.snapshot(SET) }).catch(() => {}); }, 400);
}
