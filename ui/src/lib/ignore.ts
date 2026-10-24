// Игнор: та же логика, что у узла (IgnoreRules) — для мгновенной проверки пути в окне.
import { api } from './api';
import { toast } from './state.svelte';

function globRx(p: string) {
  let s = '^';
  for (let i = 0; i < p.length; i++) {
    const c = p[i];
    if (c === '*' && p[i + 1] === '*') { if (p[i + 2] === '/') { s += '(?:.*/)?'; i += 2; } else { s += '.*'; i++; } }
    else if (c === '*') s += '[^/]*';
    else if (c === '?') s += '[^/]';
    else s += c.replace(/[.+^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(s + '$', 'i');
}

export function localMatch(patterns: string[], path: string): string | null {
  path = path.replace(/\\/g, '/').replace(/^\/+/, '');
  for (const raw of patterns) {
    const p = raw.trim().replace(/\\/g, '/');
    if (!p || p.startsWith('#')) continue;
    const dirOnly = p.endsWith('/'), body = p.replace(/^\/+|\/+$/g, ''), anchored = p.startsWith('/') || body.includes('/');
    if (!anchored) {
      const parts = path.split('/'), last = dirOnly ? parts.length - 1 : parts.length, rx = globRx(body);
      for (let i = 0; i < last; i++) if (rx.test(parts[i])) return p;
    } else if ((!dirOnly && globRx(body).test(path)) || globRx(body + '/**').test(path)) return p;
  }
  return null;
}

export async function igChange(mode: string, body: any) {
  const r = await api('POST', mode === 'set' ? 'ignore' : 'ignore/' + mode, body);
  if (r.warning) toast(r.warning, 'bad');
  return r;
}
