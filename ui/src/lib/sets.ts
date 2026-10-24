// Привязка готового HTML к настройкам: [data-set="ключ"] — флажок, ползунок, список или .seg с кнопками [data-v].
// Для панелей, которые рисуются без Svelte (доска).
import { SET, setSet } from './state.svelte';

export function bindSets(root: Element, after?: (k: string) => void) {
  root.querySelectorAll<HTMLElement>('[data-set]').forEach(el => {
    const k = el.dataset.set!;
    if (el.classList.contains('seg')) {
      el.querySelectorAll<HTMLElement>('[data-v]').forEach(b => {
        b.classList.toggle('on', String(SET[k]) === b.dataset.v);
        b.onclick = () => {
          const v = b.dataset.v !== '' && !isNaN(+b.dataset.v!) ? +b.dataset.v! : b.dataset.v;
          setSet(k, v);
          el.querySelectorAll<HTMLElement>('[data-v]').forEach(x => x.classList.toggle('on', x === b));
          after && after(k);
        };
      });
    } else if ((el as HTMLInputElement).type === 'checkbox') {
      const i = el as HTMLInputElement;
      i.checked = !!SET[k];
      i.onchange = () => { setSet(k, i.checked); after && after(k); };
    } else if ((el as HTMLInputElement).type === 'range') {
      const i = el as HTMLInputElement;
      i.value = String(SET[k]);
      i.oninput = () => { setSet(k, +i.value); after && after(k); };
    } else if (el.tagName === 'SELECT') {
      const i = el as HTMLSelectElement;
      i.value = String(SET[k]);
      i.onchange = () => { setSet(k, i.value); after && after(k); };
    }
  });
}
