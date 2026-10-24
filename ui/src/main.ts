import './styles.css';
import './extra.css';
import './fonts/fonts.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { loadSettings, ui, SET } from './lib/state.svelte';
import { R, V, ROSTER } from './voice/engine.svelte';
import { CHT, MSGS } from './lib/chat.svelte';

// для проверок и отладки из консоли окна
(window as any).__b = { ui, SET, R, V, ROSTER, CHT, MSGS };

// настройки этого ПК хранит мост — сначала их, потом панель (тема, громкости, устройства)
loadSettings().finally(() => {
  mount(App, { target: document.getElementById('app')! });
});
