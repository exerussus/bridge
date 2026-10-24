<script lang="ts">
  // Окно с готовым HTML (для модулей без Svelte — доска): mount навешивает обработчики.
  let { html, mount }: { html: string; mount?: (el: HTMLElement) => void } = $props();
  function init(el: HTMLElement) { mount && mount(el); }
</script>

<div use:init>{@html html}</div>
