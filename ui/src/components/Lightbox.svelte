<script lang="ts">
  import { LB, closeLightbox } from '../lib/lightbox.svelte';
  import { api } from '../lib/api';
  import { toast } from '../lib/state.svelte';
  import { STK } from '../lib/chat.svelte';
  async function keep(e: Event) {
    e.stopPropagation();
    try { await api('POST', 'stickers/keep', { id: LB.cur!.sticker }); STK.list = null; toast('Стикер добавлен в «Мои»', 'ok'); } catch (err: any) { toast(err.message, 'bad'); }
    closeLightbox();
  }
  $effect(() => {
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape' && LB.cur) closeLightbox(); };
    addEventListener('keydown', k);
    return () => removeEventListener('keydown', k);
  });
</script>

{#if LB.cur}
  <div class="lightbox" role="presentation" onclick={e => { if (!(e.target as HTMLElement).closest('a')) closeLightbox(); }}>
    <img src={LB.cur.src} alt="" />
    <div class="row">
      {#if LB.cur.sticker}<button class="b sm pri" onclick={keep}>В мои стикеры</button>{/if}
      <a class="b sm" href={LB.cur.dl} download={LB.cur.name}>Сохранить</a>
      <button class="b sm">Закрыть</button>
    </div>
  </div>
{/if}
