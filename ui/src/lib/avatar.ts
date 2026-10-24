// Аватарка: картинка → квадрат 256×256 по центру → JPEG; гифка уходит как есть (иначе пропадёт анимация).
import { api } from './api';
import { toast } from './state.svelte';
import { pickFiles } from './util';
import { loadAvatars } from '../voice/engine.svelte';

export async function avatarPick() {
  const [f] = await pickFiles({ accept: 'image/*' });
  if (!f) return;
  if (f.type === 'image/gif' || /\.gif$/i.test(f.name)) {
    if (f.size > 2 * 1048576) { toast('Гифка ' + (f.size / 1048576).toFixed(1) + ' МБ — для аватарки нужна до 2 МБ', 'bad'); return; }
    try { await api('POST', 'avatar', f, true); toast('Гифка на аватарке — соседи увидят её через несколько секунд', 'ok'); await loadAvatars(); } catch (e: any) { toast('Аватарка: ' + e.message, 'bad'); }
    return;
  }
  try {
    const img = await createImageBitmap(f), side = Math.min(img.width, img.height), c = document.createElement('canvas');
    c.width = c.height = 256;
    c.getContext('2d')!.drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, 256, 256);
    const blob = await new Promise<Blob>(r => c.toBlob(b => r(b!), 'image/jpeg', 0.88));
    await api('POST', 'avatar', blob, true);
    toast('Аватарка обновлена — соседи увидят её через несколько секунд', 'ok');
    await loadAvatars();
  } catch (e: any) { toast('Аватарка: ' + e.message, 'bad'); }
}
export async function avatarRemove() { try { await api('POST', 'avatar/remove'); await loadAvatars(); } catch (e: any) { toast(e.message, 'bad'); } }

export const keyName = (c: string) =>
  ({ Space: 'Пробел', ShiftLeft: 'Левый Shift', ShiftRight: 'Правый Shift', ControlLeft: 'Левый Ctrl', ControlRight: 'Правый Ctrl', AltLeft: 'Левый Alt', AltRight: 'Правый Alt', CapsLock: 'Caps Lock', Backquote: 'Ё / `' } as Record<string, string>)[c] ||
  String(c || '').replace(/^Key/, '').replace(/^Digit/, '').replace(/^Numpad/, 'Num ');
