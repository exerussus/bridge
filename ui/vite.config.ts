import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Панель моста. Собранное (dist/) узел встраивает в себя и раздаёт: в окне приложения — по протоколу bridge://,
// в браузере — с 127.0.0.1. Файлы сборки лежат в /app/, чтобы не пересекаться с /assets/ узла (шрифты, окно показа).
export default defineConfig({
  plugins: [svelte()],
  base: '/',
  build: { outDir: 'dist', assetsDir: 'app', emptyOutDir: true, target: 'es2022', chunkSizeWarningLimit: 2000, sourcemap: false },
  server: {
    port: 5173,
    // npm run dev: панель с горячей перезагрузкой, запросы — в запущенный мост (токен — из bridge-data/local.token)
    proxy: { '/bridge': 'http://127.0.0.1:5390', '/assets': 'http://127.0.0.1:5390', '/favicon.svg': 'http://127.0.0.1:5390' },
  },
});
