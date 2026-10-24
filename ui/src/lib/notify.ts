// Уведомления, когда окно свёрнуто: в приложении — системные (из значка у часов, щелчок откроет нужную вкладку),
// в браузере — уведомления браузера, если их включили кнопкой 🔔.
import { IS_APP, app } from './api';
import { SET, setTab } from './state.svelte';

export function notifyTo(title: string, body: string, tab: string) {
  if (IS_APP) { app('notify', { title, body, tab }); return; }
  if (!SET.notify || !('Notification' in window) || Notification.permission !== 'granted') return;
  try {
    const n = new Notification(title, { body, icon: '/favicon.svg', tag: 'bridge-' + tab });
    n.onclick = () => { try { window.focus(); } catch {} setTab(tab); n.close(); };
  } catch {}
}
