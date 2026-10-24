import { ui, setTab } from './state.svelte';
export function goSettings(sub: string) { setTab('settings'); ui.stTab = sub; }
