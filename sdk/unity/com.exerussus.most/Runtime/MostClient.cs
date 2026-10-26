using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Threading.Tasks;

namespace Exerussus.Most
{
    /// <summary>
    /// Связь приложения с мостом этого ПК. Все события приходят в главном потоке Unity (из Most.Pump, его зовёт MostRunner).
    /// Обычно не создаётся вручную: Most.Connect().
    /// </summary>
    public sealed class MostClient
    {
        public enum Status { Connecting, Open, Closed }

        readonly IMostLink link;
        readonly ConcurrentQueue<MostIncoming> inbox = new ConcurrentQueue<MostIncoming>();
        readonly Dictionary<long, (TaskCompletionSource<object> tcs, long until)> pending = new Dictionary<long, (TaskCompletionSource<object>, long)>();
        readonly TaskCompletionSource<bool> opened = new TaskCompletionSource<bool>();
        readonly Stopwatch clock = Stopwatch.StartNew();
        readonly System.Random rnd = new System.Random();
        readonly List<(long due, MostIncoming m)> delayed = new List<(long, MostIncoming)>();
        long seq, lastReliableDue, lastTimeoutCheck;

        public Status State { get; private set; } = Status.Connecting;
        /// <summary>Почему закрыто (для человека).</summary>
        public string CloseReason { get; private set; } = "";
        /// <summary>ID приложения и имя моста (ПК).</summary>
        public string App { get; private set; } = "";
        public string NodeName { get; private set; } = "";
        /// <summary>Голосовой канал хозяина ПК: сеансы — только с теми, кто в нём.</summary>
        public MostChannel Channel { get; private set; } = new MostChannel();
        public MostSession Session { get; }
        public MostActivity Activity { get; }
        /// <summary>Имитация плохой сети (входящее). Меняется на лету.</summary>
        public MostSimulation Simulation { get; set; } = new MostSimulation();

        /// <summary>Связь готова.</summary>
        public event Action OnOpen;
        /// <summary>Связь закрыта: причина.</summary>
        public event Action<string> OnClosed;
        /// <summary>Хозяин ПК зашёл в другой канал или вышел из канала.</summary>
        public event Action<MostChannel> OnChannel;
        /// <summary>Ошибка от моста (сообщение не прошло и т. п.).</summary>
        public event Action<string> OnError;

        /// <summary>Завершается, когда связь готова (или исключение MostException с причиной).</summary>
        public Task WhenOpen => opened.Task;

        internal MostClient(IMostLink link)
        {
            this.link = link;
            Session = new MostSession(this);
            Activity = new MostActivity(this);
            link.Open(inbox);
        }

        long Now => clock.ElapsedMilliseconds;

        // ───────── отправка ─────────

        /// <summary>Команда моста (op) с полями; ответ — значение v или MostException.</summary>
        internal Task<object> Request(string op, Dictionary<string, object> fields = null)
        {
            if (State == Status.Closed) return Task.FromException<object>(new MostException("связь с мостом закрыта: " + CloseReason));
            var re = ++seq;
            var tcs = new TaskCompletionSource<object>(TaskCreationOptions.RunContinuationsAsynchronously);
            pending[re] = (tcs, Now + 15000);
            var d = fields ?? new Dictionary<string, object>();
            d["op"] = op;
            d["re"] = re;
            link.SendText(MostJson.Serialize(d));
            return tcs.Task;
        }

        /// <summary>Вызов вне сеанса: me, store.get, store.set, notify, toast, ticket.</summary>
        internal Task<object> Call(string method, params object[] args)
        {
            var re = ++seq;
            var list = new List<object>();
            foreach (var a in args) list.Add(MostJson.ToTree(a));
            var t = link.Call(re, method, list);
            if (t != null) return t;
            var tcs = new TaskCompletionSource<object>(TaskCreationOptions.RunContinuationsAsynchronously);
            pending[re] = (tcs, Now + 20000);
            return tcs.Task;
        }

        internal void SendFrame(byte[] frame)
        {
            if (State == Status.Closed) return;
            link.SendBinary(frame);
        }

        // ───────── вне сеанса ─────────

        /// <summary>Сведения о приложении: {id, name, version, granted, node, …}.</summary>
        public Task<object> Me() => Call("me");
        /// <summary>Хранилище приложения на этом ПК (до 1 МБ на всё). Значение — дерево MostJson.</summary>
        public Task<object> StoreGet(string key) => Call("store.get", key ?? "");
        public async Task<T> StoreGet<T>(string key)
        {
            var v = await StoreGet(key);
            return v == null ? default : UnityEngine.JsonUtility.FromJson<T>(MostJson.Serialize(v));
        }
        public Task StoreSet(string key, object value) => Call("store.set", key ?? "", MostJson.ToTree(value));
        /// <summary>Уведомление (нужно право notify).</summary>
        public Task Notify(string text) => Call("notify", text ?? "");
        /// <summary>Короткая подсказка в панели моста.</summary>
        public Task Toast(string text) => Call("toast", text ?? "");
        /// <summary>
        /// Проверить билет участника — для своего сервера на ПК хоста (Transport = "external"):
        /// {ok, who: {sid, app, cid, node, name}} или MostException.
        /// </summary>
        public Task<object> CheckTicket(string ticket) => Call("ticket", ticket ?? "");

        public void Close()
        {
            if (State == Status.Closed) return;
            link.Close();
            Closed("закрыто приложением");
        }

        // ───────── приём (главный поток) ─────────

        internal void Pump()
        {
            link.Pump();
            var sim = Simulation;
            var now = Now;
            while (inbox.TryDequeue(out var m))
            {
                if (sim == null || !sim.Enabled || m.Closed != null) { Dispatch(m); continue; }
                var lossy = m.Bin != null && m.Bin.Length > 0 && m.Bin[0] != (byte)MostDelivery.Reliable;
                if (lossy && rnd.NextDouble() < sim.Loss) continue;
                var due = now + Math.Max(0, sim.LatencyMs + (sim.JitterMs > 0 ? rnd.Next(-sim.JitterMs, sim.JitterMs + 1) : 0));
                if (!lossy) { due = Math.Max(due, lastReliableDue); lastReliableDue = due; }
                delayed.Add((due, m));
            }
            if (delayed.Count > 0)
            {
                // стабильно по времени: надёжное сохраняет порядок
                var ready = new List<(long due, MostIncoming m)>();
                for (var i = 0; i < delayed.Count; i++) if (delayed[i].due <= now) ready.Add(delayed[i]);
                if (ready.Count > 0)
                {
                    delayed.RemoveAll(x => x.due <= now);
                    ready.Sort((a, b) => a.due.CompareTo(b.due));
                    foreach (var r in ready) Dispatch(r.m);
                }
            }
            if (now - lastTimeoutCheck > 1000)
            {
                lastTimeoutCheck = now;
                List<long> late = null;
                foreach (var kv in pending) if (kv.Value.until < now) (late ??= new List<long>()).Add(kv.Key);
                if (late != null)
                    foreach (var re in late) { var p = pending[re]; pending.Remove(re); p.tcs.TrySetException(new MostException("мост не ответил вовремя")); }
            }
        }

        void Dispatch(MostIncoming m)
        {
            if (m.Closed != null) { Closed(m.Closed); return; }
            if (m.Bin != null)
            {
                if (m.Bin.Length < 3) return;
                var from = (ushort)(m.Bin[1] | (m.Bin[2] << 8));
                var d = (MostDelivery)Math.Min(m.Bin[0], (byte)2);
                Session.HandleData(new MostData(from, d, new ArraySegment<byte>(m.Bin, 3, m.Bin.Length - 3)));
                return;
            }
            object v;
            try { v = MostJson.Parse(m.Text); }
            catch (Exception e) { UnityEngine.Debug.LogWarning("[Мост] непонятное сообщение: " + e.Message); return; }
            var ev = MostJson.Str(v, "ev");
            switch (ev)
            {
                case "re":
                {
                    var re = MostJson.Long(v, "re");
                    if (!pending.TryGetValue(re, out var p)) return;
                    pending.Remove(re);
                    if (MostJson.Bool(v, "ok")) p.tcs.TrySetResult(MostJson.Get(v, "v"));
                    else p.tcs.TrySetException(new MostException(MostJson.Str(v, "error", "ошибка моста")));
                    return;
                }
                case "hello":
                    App = MostJson.Str(v, "app", App);
                    NodeName = MostJson.Str(v, "node", NodeName);
                    if (MostJson.Get(v, "channel") != null) Channel = MostChannel.From(MostJson.Get(v, "channel"));
                    if (State == Status.Connecting)
                    {
                        State = Status.Open;
                        opened.TrySetResult(true);
                        Safe(() => OnOpen?.Invoke());
                    }
                    return;
                case "channel":
                {
                    var c = MostChannel.From(v);
                    if (c.Members.Count == 0 && c.Room == Channel.Room) c.Members = Channel.Members;
                    Channel = c;
                    Safe(() => OnChannel?.Invoke(c));
                    return;
                }
                case "activity":
                    Activity.Handle(MostJson.Get(v, "e"));
                    return;
                case "error":
                    var text = MostJson.Str(v, "error");
                    UnityEngine.Debug.LogWarning("[Мост] " + text);
                    Safe(() => OnError?.Invoke(text));
                    return;
                default:
                    Session.HandleEvent(ev, v);
                    return;
            }
        }

        void Closed(string why)
        {
            if (State == Status.Closed) return;
            State = Status.Closed;
            CloseReason = why;
            var ex = new MostException(why);
            opened.TrySetException(ex);
            foreach (var p in pending.Values) p.tcs.TrySetException(ex);
            pending.Clear();
            delayed.Clear();
            Session.HandleClosed(why);
            Safe(() => OnClosed?.Invoke(why));
        }

        internal static void Safe(Action a)
        {
            try { a(); }
            catch (Exception e) { UnityEngine.Debug.LogException(e); }
        }
    }

    /// <summary>
    /// Журнал активности хозяина ПК в мосте (право activity): минуты в канале и разговора, сообщения, показ экрана,
    /// пинги, отправки. Только факты, без содержимого. Игре — начислять награды, в том числе за время, пока она была закрыта:
    /// храните cursor (StoreSet) и забирайте Since(cursor) при запуске.
    /// </summary>
    public sealed class MostActivity
    {
        readonly MostClient c;
        internal MostActivity(MostClient c) { this.c = c; }

        /// <summary>Новое событие: {seq, at, kind, …} — kind: voice.minute (talkSec), chat.message, screen.minute, board.edit, ping, files.sent, channel.join.</summary>
        public event Action<object> OnEvent;

        /// <summary>События после cursor: {events: [...], cursor, more}.</summary>
        public Task<object> Since(long cursor, int limit = 500) =>
            c.Request("activity.since", new Dictionary<string, object> { ["cursor"] = cursor, ["limit"] = (long)limit });

        /// <summary>Итоги: period — "today" или "week": {voiceMinutes, talkMinutes, talkSeconds, counts}.</summary>
        public Task<object> Stats(string period = "today") =>
            c.Request("activity.stats", new Dictionary<string, object> { ["period"] = period });

        internal void Handle(object e) => MostClient.Safe(() => OnEvent?.Invoke(e));
    }
}
