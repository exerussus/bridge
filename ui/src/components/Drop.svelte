<script lang="ts">
  // Рамка «перетащите сюда или нажмите»: файлы и папки целиком (путь — от перетащенной папки).
  import type { Snippet } from 'svelte';
  import { dropFiles, pickFiles } from '../lib/util';
  let { onfiles, children, style = '', id = '', multiple = true, accept = '' }:
    { onfiles: (list: { file: File; path: string }[]) => void; children: Snippet; style?: string; id?: string; multiple?: boolean; accept?: string } = $props();
  let over = $state(false);
  async function pick() {
    const f = await pickFiles({ multiple, accept });
    if (f.length) onfiles(f.map(file => ({ file, path: file.name })));
  }
</script>

<div class="drop" class:over {id} {style} role="button" tabindex="0"
  onclick={pick} onkeydown={e => e.key === 'Enter' && pick()}
  ondragenter={e => { e.preventDefault(); over = true; }}
  ondragover={e => { e.preventDefault(); over = true; }}
  ondragleave={e => { e.preventDefault(); over = false; }}
  ondrop={async e => { e.preventDefault(); over = false; onfiles(await dropFiles(e)); }}>
  {@render children()}
</div>
