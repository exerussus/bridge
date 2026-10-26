// Мост — SDK веб-приложения (apps@1). Подключение: <script src="/most/sdk.js"></script> → window.most.
// Приложение живёт в изолированной рамке и в мост ходит только через панель (postMessage): права проверяет мост.
(function () {
  'use strict';
  if (window.most) return;
  var target = window.parent !== window ? window.parent : window.opener || null;
  var pending = {}, seq = 0, handlers = {};
  function call(m) {
    var args = Array.prototype.slice.call(arguments, 1);
    return new Promise(function (resolve, reject) {
      if (!target) { reject(new Error('Приложение открыто вне моста')); return; }
      var id = ++seq;
      pending[id] = { resolve: resolve, reject: reject };
      target.postMessage({ most: 1, id: id, m: m, a: args }, '*');
    });
  }
  window.addEventListener('message', function (e) {
    if (e.source !== target) return;
    var d = e.data;
    if (!d || d.most !== 1) return;
    if (d.re) {
      var p = pending[d.re];
      if (!p) return;
      delete pending[d.re];
      if (d.err) p.reject(new Error(d.err)); else p.resolve(d.v);
      return;
    }
    if (d.ev) (handlers[d.ev] || []).slice().forEach(function (f) { try { f(d.v); } catch (err) { console.error(err); } });
  });
  function on(ev, f) {
    (handlers[ev] = handlers[ev] || []).push(f);
    return function () { handlers[ev] = (handlers[ev] || []).filter(function (x) { return x !== f; }); };
  }
  // ── сеансы (3.5): сеть с теми, кто в том же голосовом канале ──
  var S = { sid: '', you: -1, isHost: false, host: '', members: [], state: {}, rev: 0, connect: '', ticket: '', offset: 0, rtt: {}, room: '', transport: 'bridge' };
  var netReady = null;
  function emit(ev, v) { (handlers[ev] || []).slice().forEach(function (f) { try { f(v); } catch (err) { console.error(err); } }); }
  function applyOp(st, o) {
    var path = Array.isArray(o.path) ? o.path.map(String) : (o.path ? String(o.path).split('.') : []);
    if (!path.length) { if (o.op === 'set') return o.value && typeof o.value === 'object' ? o.value : {}; if (o.op === 'merge') return merge(st, o.value); return st; }
    var cur = st;
    for (var i = 0; i < path.length - 1; i++) { if (!cur[path[i]] || typeof cur[path[i]] !== 'object') cur[path[i]] = {}; cur = cur[path[i]]; }
    var k = path[path.length - 1];
    if (o.op === 'set') cur[k] = o.value; else if (o.op === 'del') delete cur[k]; else if (o.op === 'merge') cur[k] = merge(cur[k] && typeof cur[k] === 'object' ? cur[k] : {}, o.value);
    return st;
  }
  function merge(a, b) {
    if (!b || typeof b !== 'object' || Array.isArray(b)) return b;
    if (!a || typeof a !== 'object') a = {};
    for (var k in b) { if (b[k] === null) delete a[k]; else a[k] = merge(a[k], b[k]); }
    return a;
  }
  function joined(v) {
    S.sid = v.sid; S.you = v.you; S.isHost = !!v.isHost; S.host = v.host; S.members = v.members || []; S.state = v.state || {}; S.rev = v.rev || 0;
    S.connect = v.connect || ''; S.ticket = v.ticket || ''; S.room = v.room || ''; S.transport = v.transport || 'bridge';
    S.offset = typeof v.hostNow === 'number' ? v.hostNow - Date.now() : 0;
    emit('joined', v);
    return v;
  }
  function left() { S.sid = ''; S.you = -1; S.isHost = false; S.members = []; }
  on('net', function (e) {
    switch (e.ev) {
      case 'member':
        if (e.op === 'join') S.members.push(e.m); else S.members = S.members.filter(function (m) { return m.cid !== e.cid; });
        emit(e.op === 'join' ? 'member' : 'left', e.op === 'join' ? e.m : e); break;
      case 'state': for (var i = 0; i < (e.ops || []).length; i++) S.state = applyOp(S.state, e.ops[i]); S.rev = e.rev; emit('state', e); break;
      case 'presence': emit('presence', e); break;
      case 'connect': S.connect = e.value; emit('connect', e.value); break;
      case 'clock': S.offset = e.offset; S.rtt.host = e.rtt; emit('clock', e); break;
      case 'rtt': S.rtt = Object.assign(S.rtt, e.rtt); emit('rtt', e.rtt); break;
      case 'ended': left(); emit('ended', e.why || ''); break;
      case 'sessions': emit('sessions', e.list); break;
      case 'channel': emit('channel', e); break;
      case 'closed': left(); netReady = null; emit('ended', 'связь с мостом закрыта'); break;
      case 'error': console.warn('мост:', e.error); emit('error', e.error); break;
    }
  });
  on('netdata', function (d) {
    var data = d.data;
    emit('data', { from: d.from, delivery: ['reliable', 'fast', 'latest'][d.delivery] || 'reliable', data: data,
      text: function () { return new TextDecoder().decode(data); }, json: function () { return JSON.parse(new TextDecoder().decode(data)); } });
  });
  function net() { if (!netReady) netReady = call('net.connect').catch(function (e) { netReady = null; throw e; }); return netReady; }
  function nop(o) { return net().then(function () { return call('net.op', o); }); }
  var session = {
    HOST: 0, ALL: 0xFFFF, OTHERS: 0xFFFE,
    /** События: joined, member, left, data, presence, state, connect, clock, rtt, ended, sessions, channel, error. */
    on: on,
    /** Войти в идущий сеанс этого приложения в вашем канале или начать свой. opts: {max, meta, state, transport}. */
    joinOrCreate: function (opts) { return nop(Object.assign({ op: 'joinOrCreate' }, opts || {})).then(joined); },
    create: function (opts) { return nop(Object.assign({ op: 'create' }, opts || {})).then(joined); },
    join: function (sid) { return nop({ op: 'join', sid: sid }).then(joined); },
    list: function () { return nop({ op: 'list' }); },
    leave: function () { left(); return nop({ op: 'leave' }); },
    channel: function () { return nop({ op: 'channel' }); },
    /** Отправить: to — 0 хост, N участник, most.session.ALL / OTHERS; delivery — reliable | fast | latest; key (0–255) — для latest. */
    send: function (to, data, delivery, key) { return net().then(function () { return call('net.send', to, delivery || 'reliable', data, key || 0); }); },
    sendJson: function (to, obj, delivery, key) { return session.send(to, JSON.stringify(obj), delivery, key); },
    presence: function (data) { return nop({ op: 'presence', data: data }); },
    state: {
      get: function () { return S.state; },
      set: function (path, value) { return nop({ op: 'state', ops: [{ op: 'set', path: path, value: value }] }); },
      merge: function (path, value) { return nop({ op: 'state', ops: [{ op: 'merge', path: path, value: value }] }); },
      del: function (path) { return nop({ op: 'state', ops: [{ op: 'del', path: path }] }); },
      apply: function (ops) { return nop({ op: 'state', ops: ops }); },
    },
    /** Своему серверу: строка подключения (задаёт хост) и билет участника. */
    setConnect: function (value) { return nop({ op: 'connect', value: String(value) }); },
    kick: function (cid) { return nop({ op: 'kick', cid: cid }); },
    /** Общее время сеанса (часы хоста), мс. */
    time: function () { return Date.now() + S.offset; },
    rtt: function (cid) { return cid == null ? S.rtt.host || 0 : S.rtt[cid] || 0; },
    get id() { return S.sid; }, get me() { return S.you; }, get isHost() { return S.isHost; }, get members() { return S.members.slice(); },
    get connect() { return S.connect; }, get ticket() { return S.ticket; }, get room() { return S.room; },
  };
  var activity = {
    /** События после cursor: {events, cursor, more}. */
    since: function (cursor, limit) { return call('activity.since', cursor || 0, limit || 500); },
    stats: function (period) { return call('activity.stats', period || 'today'); },
    on: function (f) { return on('activity', f); },
  };

  window.most = {
    version: 'apps@1',
    /** Низкий уровень для своих обёрток (Unity WebGL): вызов панели и события как есть. */
    _raw: { call: call, on: on },
    session: session,
    activity: activity,
    /** Сведения о приложении и мосте: {id, name, version, node, theme, granted}. */
    ready: function () { return call('ready'); },
    /** События: theme, file (файл перетащили на вкладку), visible. */
    on: on,
    store: {
      get: function (key) { return call('store.get', key || ''); },
      set: function (key, value) { return call('store.set', key, value === undefined ? null : value); },
    },
    files: {
      /** Файлы, которые выберет человек: [File]. opts: {multiple, accept}. */
      pick: function (opts) { return call('files.pick', opts || {}); },
      /** Сохранить: data — строка, ArrayBuffer или Blob. */
      save: function (name, data) { return call('files.save', String(name || 'file'), data); },
    },
    service: {
      /** Запрос к своей службе через мост (служба запустится, если не запущена). Ответ — как у fetch. */
      fetch: function (path, init) {
        init = init || {};
        return call('service.fetch', String(path || ''), { method: init.method || 'GET', body: init.body === undefined ? null : init.body, contentType: (init.headers && (init.headers['Content-Type'] || init.headers['content-type'])) || '' })
          .then(function (r) { return new Response(r.body, { status: r.status, headers: { 'Content-Type': r.contentType || 'application/octet-stream' } }); });
      },
      status: function () { return call('service.status'); },
      restart: function () { return call('service.restart'); },
    },
    notify: function (text) { return call('notify', String(text)); },
    toast: function (text) { return call('toast', String(text)); },
    window: {
      title: function (t) { return call('window.title', String(t)); },
      badge: function (n) { return call('window.badge', n == null ? '' : String(n)); },
      popout: function () { return call('window.popout'); },
    },
  };
})();
