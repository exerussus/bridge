<script lang="ts">
  // Фон плитки участника: картинка (или гифка) с вписыванием, маской, прозрачностью, размытием и оттенком.
  import { TILE_DEFAULT, maskOf, type TileStyle } from '../lib/tile.svelte';
  let { st, url = '' }: { st: TileStyle | null | undefined; url?: string } = $props();
  const s = $derived({ ...TILE_DEFAULT, ...(st || {}) });
  const mask = $derived(maskOf(s));
  const wrap = $derived(`opacity:${s.opacity / 100};${mask !== 'none' ? `mask-image:${mask};-webkit-mask-image:${mask};` : ''}`);
  const pic = $derived(`object-fit:${s.fit === 'contain' ? 'contain' : 'cover'};object-position:${s.x}% ${s.y}%;transform:scale(${(s.zoom / 100) * (s.blur ? 1.06 : 1)});transform-origin:${s.x}% ${s.y}%;${s.blur ? `filter:blur(${s.blur}px);` : ''}`);
  const rep = $derived(`background-image:url("${url}");background-size:${Math.round((120 * s.zoom) / 100)}px;background-position:${s.x}% ${s.y}%;${s.blur ? `filter:blur(${s.blur}px);` : ''}`);
</script>

{#if st && (url || (s.tint && s.tintAmt))}
  <div class="tart" style={wrap} aria-hidden="true">
    {#if url}{#if s.fit === 'repeat'}<div class="trep" style={rep}></div>{:else}<img alt="" src={url} style={pic} draggable="false" />{/if}{/if}
    {#if s.tint && s.tintAmt}<div class="ttint" style="background:{s.tint};opacity:{s.tintAmt / 100}"></div>{/if}
  </div>
{/if}
