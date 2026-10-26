// Мост — WebGL: связь сборки Unity с панелью моста через window.most (SDK /most/sdk.js в index.html).
var MostWebLib = {
  $MOST: { q: [], open: false },

  MostWeb_Available: function () {
    return (typeof window !== 'undefined' && window.most && window.most._raw) ? 1 : 0;
  },

  MostWeb_Open__deps: ['$MOST'],
  MostWeb_Open: function () {
    var raw = window.most._raw;
    var push = function (v) { MOST.q.push(v); };
    if (!MOST.open) {
      MOST.open = true;
      raw.on('net', function (e) {
        if (e && e.ev === 'closed') push('!closed:связь с мостом закрыта');
        else push(JSON.stringify(e));
      });
      raw.on('netdata', function (d) {
        var p = d.data instanceof Uint8Array ? d.data : new Uint8Array(d.data || []);
        var f = new Uint8Array(p.length + 3);
        f[0] = d.delivery & 255; f[1] = d.from & 255; f[2] = (d.from >> 8) & 255; f.set(p, 3);
        push(f);
      });
    }
    raw.call('net.connect').then(function (v) {
      push(JSON.stringify(Object.assign({ ev: 'hello' }, v && typeof v === 'object' ? v : {})));
    }, function (err) { push('!closed:' + String(err && err.message || err)); });
  },

  MostWeb_SendText__deps: ['$MOST'],
  MostWeb_SendText: function (ptr) {
    var o = JSON.parse(UTF8ToString(ptr)), re = o.re;
    delete o.re;
    var m = o.op === 'activity.since' ? 'activity.since' : o.op === 'activity.stats' ? 'activity.stats' : 'net.op';
    var p = m === 'net.op' ? window.most._raw.call('net.op', o)
      : m === 'activity.since' ? window.most._raw.call('activity.since', o.cursor || 0, o.limit || 500)
      : window.most._raw.call('activity.stats', o.period || 'today');
    p.then(function (v) { if (re != null) MOST.q.push(JSON.stringify({ ev: 're', re: re, ok: true, v: v === undefined ? null : v })); },
      function (e) { if (re != null) MOST.q.push(JSON.stringify({ ev: 're', re: re, ok: false, error: String(e && e.message || e) })); });
  },

  MostWeb_SendBin: function (ptr, len) {
    var b = HEAPU8.slice(ptr, ptr + len);
    if (b.length < 3) return;
    var dl = b[0], to = b[1] | (b[2] << 8), key = 0, off = 3;
    if (dl === 2) { key = b[3] || 0; off = 4; }
    window.most._raw.call('net.send', to, ['reliable', 'fast', 'latest'][dl] || 'reliable', b.slice(off), key).catch(function (e) {
      MOST.q.push(JSON.stringify({ ev: 'error', error: String(e && e.message || e) }));
    });
  },

  MostWeb_Call__deps: ['$MOST'],
  MostWeb_Call: function (re, mptr, aptr) {
    var m = UTF8ToString(mptr), a = JSON.parse(UTF8ToString(aptr) || '[]');
    var name = m === 'me' ? 'ready' : m;
    window.most._raw.call.apply(null, [name].concat(a)).then(
      function (v) { MOST.q.push(JSON.stringify({ ev: 're', re: re, ok: true, v: v === undefined ? null : v })); },
      function (e) { MOST.q.push(JSON.stringify({ ev: 're', re: re, ok: false, error: String(e && e.message || e) })); });
  },

  MostWeb_Peek__deps: ['$MOST'],
  MostWeb_Peek: function () {
    if (!MOST.q.length) return -1;
    var x = MOST.q[0];
    return typeof x === 'string' ? -2 : x.length;
  },

  MostWeb_NextText__deps: ['$MOST'],
  MostWeb_NextText: function () {
    var s = MOST.q.length && typeof MOST.q[0] === 'string' ? MOST.q.shift() : '';
    var size = lengthBytesUTF8(s) + 1, buf = _malloc(size);
    stringToUTF8(s, buf, size);
    return buf;
  },

  MostWeb_NextBin__deps: ['$MOST'],
  MostWeb_NextBin: function (ptr, cap) {
    var x = MOST.q.shift();
    if (x && typeof x !== 'string') HEAPU8.set(x.subarray(0, cap), ptr);
  },

  MostWeb_Close__deps: ['$MOST'],
  MostWeb_Close: function () {
    try { window.most._raw.call('net.close'); } catch (e) {}
  },
};
autoAddDeps(MostWebLib, '$MOST');
mergeInto(LibraryManager.library, MostWebLib);
