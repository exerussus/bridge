using System;
using System.Collections.Generic;
using System.Text;
using System.Threading.Tasks;

namespace Exerussus.Most
{
    /// <summary>
    /// Сеанс приложения с теми, кто в том же голосовом канале. Звезда: хост (cid 0) — ПК, начавший сеанс; остальные
    /// ходят к нему через свои мосты. Что слать и как разбирать — решает приложение (байты, свои форматы).
    /// </summary>
    public sealed class MostSession
    {
        public const int ReliableMax = 1024 * 1024;
        public const int FastMax = 1200;

        internal readonly MostClient Client;
        readonly List<MostMember> members = new List<MostMember>();
        readonly Dictionary<ushort, int> rtt = new Dictionary<ushort, int>();
        long offset;

        public MostState State { get; }
        /// <summary>Пусто — не в сеансе.</summary>
        public string Id { get; private set; } = "";
        public bool InSession => Id.Length > 0;
        /// <summary>Мой номер в сеансе (0 — я хост).</summary>
        public ushort Me { get; private set; }
        public bool IsHost { get; private set; }
        /// <summary>Мост (ПК) хоста.</summary>
        public string Host { get; private set; } = "";
        public string Room { get; private set; } = "";
        public string Title { get; private set; } = "";
        public int Max { get; private set; }
        /// <summary>"bridge" или "external".</summary>
        public string Transport { get; private set; } = "bridge";
        /// <summary>Строка подключения к своему серверу (задаёт хост, при Transport = "external").</summary>
        public string Connect { get; private set; } = "";
        /// <summary>Мой билет: передайте своему серверу, он проверит его у моста хоста (CheckTicket).</summary>
        public string Ticket { get; private set; } = "";
        public object Meta { get; private set; }
        public IReadOnlyList<MostMember> Members => members;

        /// <summary>Вошёл в сеанс (свой или чужой).</summary>
        public event Action OnJoined;
        public event Action<MostMember> OnMember;
        /// <summary>Участник вышел: cid, причина.</summary>
        public event Action<ushort, string> OnLeft;
        /// <summary>Сообщение от участника. Буфер действителен только внутри обработчика.</summary>
        public event Action<MostData> OnData;
        /// <summary>Присутствие участника (до 4 КБ): что он делает, куда смотрит и т. п. Не хранится.</summary>
        public event Action<ushort, object> OnPresence;
        public event Action<string> OnConnect;
        /// <summary>Сеанс закончился для меня: причина (хост вышел, вышли из канала, убрали…).</summary>
        public event Action<string> OnEnded;
        /// <summary>Список сеансов в канале изменился.</summary>
        public event Action<List<MostSessionInfo>> OnSessions;
        /// <summary>Обновились задержки (мс) до участников.</summary>
        public event Action OnRtt;

        internal MostSession(MostClient c)
        {
            Client = c;
            State = new MostState(this);
        }

        // ───────── вход и выход ─────────

        /// <summary>Войти в идущий в канале сеанс этого приложения — или начать свой.</summary>
        public async Task JoinOrCreate(MostSessionOptions options = null) => Joined(await Client.Request("joinOrCreate", (options ?? new MostSessionOptions()).Fill(new Dictionary<string, object>())));
        /// <summary>Начать свой сеанс (я — хост).</summary>
        public async Task Create(MostSessionOptions options = null) => Joined(await Client.Request("create", (options ?? new MostSessionOptions()).Fill(new Dictionary<string, object>())));
        public async Task Join(string sid) => Joined(await Client.Request("join", new Dictionary<string, object> { ["sid"] = sid }));

        /// <summary>Сеансы этого приложения в моём канале.</summary>
        public async Task<List<MostSessionInfo>> List()
        {
            var r = new List<MostSessionInfo>();
            if (await Client.Request("list") is List<object> l)
                foreach (var x in l) r.Add(MostSessionInfo.From(x));
            return r;
        }

        public Task Leave()
        {
            var was = InSession;
            Reset();
            if (was) MostClient.Safe(() => OnEnded?.Invoke("вы вышли"));
            return Client.Request("leave");
        }

        /// <summary>Хост: убрать участника.</summary>
        public Task Kick(ushort cid) => Client.Request("kick", new Dictionary<string, object> { ["cid"] = (long)cid });
        /// <summary>Хост: строка подключения к своему серверу (адрес:порт и т. п.) — получат все.</summary>
        public Task SetConnect(string value) => Client.Request("connect", new Dictionary<string, object> { ["value"] = value ?? "" });
        /// <summary>Моё присутствие (до 4 КБ) — остальным.</summary>
        public Task Presence(object data) => Client.Request("presence", new Dictionary<string, object> { ["data"] = MostJson.ToTree(data) });

        // ───────── данные ─────────

        /// <summary>
        /// Отправить байты. to — MostTo.Host, MostTo.All, MostTo.Others или cid участника.
        /// Reliable — до 1 МБ, Fast и Latest — до 1200 байт; key — для Latest (своё «последнее» на каждый ключ).
        /// </summary>
        public void Send(ushort to, ArraySegment<byte> data, MostDelivery delivery = MostDelivery.Reliable, byte key = 0)
        {
            if (!InSession) throw new MostException("вы не в сеансе");
            var lim = delivery == MostDelivery.Reliable ? ReliableMax : FastMax;
            if (data.Count > lim) throw new MostException("сообщение " + data.Count + " байт — предел " + lim);
            var head = delivery == MostDelivery.Latest ? 4 : 3;
            var f = new byte[head + data.Count];
            f[0] = (byte)delivery;
            f[1] = (byte)(to & 0xFF);
            f[2] = (byte)(to >> 8);
            if (head == 4) f[3] = key;
            if (data.Count > 0) Buffer.BlockCopy(data.Array, data.Offset, f, head, data.Count);
            Client.SendFrame(f);
        }

        public void Send(ushort to, byte[] data, MostDelivery delivery = MostDelivery.Reliable, byte key = 0) => Send(to, new ArraySegment<byte>(data ?? Array.Empty<byte>()), delivery, key);
        public void SendText(ushort to, string text, MostDelivery delivery = MostDelivery.Reliable, byte key = 0) => Send(to, Encoding.UTF8.GetBytes(text ?? ""), delivery, key);
        /// <summary>Свой [Serializable] объект через JsonUtility.</summary>
        public void SendJson(ushort to, object obj, MostDelivery delivery = MostDelivery.Reliable, byte key = 0) => SendText(to, UnityEngine.JsonUtility.ToJson(obj), delivery, key);

        // ───────── время ─────────

        /// <summary>Общее время сеанса — часы хоста, мс (Unix). Для одновременных событий у всех.</summary>
        public long Time => DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() + offset;
        /// <summary>Задержка туда-обратно, мс: Rtt() — моя до хоста; Rtt(cid) — участника до хоста (у хоста — 0).</summary>
        public int Rtt(ushort cid = 0) => cid == Me && cid != MostTo.Host ? Rtt() : rtt.TryGetValue(cid, out var r) ? r : 0;
        public MostMember Member(ushort cid) { foreach (var m in members) if (m.Cid == cid) return m; return null; }

        // ───────── от моста ─────────

        void Joined(object v)
        {
            Id = MostJson.Str(v, "sid");
            Me = (ushort)MostJson.Long(v, "you");
            IsHost = MostJson.Bool(v, "isHost");
            Host = MostJson.Str(v, "host");
            Room = MostJson.Str(v, "room");
            Title = MostJson.Str(v, "title");
            Max = (int)MostJson.Long(v, "max");
            Transport = MostJson.Str(v, "transport", "bridge");
            Connect = MostJson.Str(v, "connect");
            Ticket = MostJson.Str(v, "ticket");
            Meta = MostJson.Get(v, "meta");
            var hostNow = MostJson.Long(v, "hostNow");
            offset = hostNow > 0 ? hostNow - DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() : 0;
            members.Clear();
            if (MostJson.Get(v, "members") is List<object> l) foreach (var m in l) members.Add(MostMember.From(m));
            rtt.Clear();
            State.Reset(MostJson.Get(v, "state"), MostJson.Long(v, "rev"));
            MostClient.Safe(() => OnJoined?.Invoke());
        }

        void Reset()
        {
            Id = ""; Me = 0; IsHost = false; Host = ""; Ticket = ""; Connect = "";
            members.Clear();
            rtt.Clear();
        }

        internal void HandleData(MostData d)
        {
            if (!InSession) return;
            var h = OnData;
            if (h == null) return;
            foreach (Action<MostData> f in h.GetInvocationList()) MostClient.Safe(() => f(d));
        }

        internal void HandleEvent(string ev, object v)
        {
            switch (ev)
            {
                case "member":
                    if (MostJson.Str(v, "op") == "join")
                    {
                        var m = MostMember.From(MostJson.Get(v, "m"));
                        members.RemoveAll(x => x.Cid == m.Cid);
                        members.Add(m);
                        MostClient.Safe(() => OnMember?.Invoke(m));
                    }
                    else
                    {
                        var cid = (ushort)MostJson.Long(v, "cid");
                        members.RemoveAll(x => x.Cid == cid);
                        rtt.Remove(cid);
                        MostClient.Safe(() => OnLeft?.Invoke(cid, MostJson.Str(v, "why")));
                    }
                    return;
                case "state":
                    State.Changed(v);
                    return;
                case "presence":
                    MostClient.Safe(() => OnPresence?.Invoke((ushort)MostJson.Long(v, "cid"), MostJson.Get(v, "data")));
                    return;
                case "connect":
                    Connect = MostJson.Str(v, "value");
                    MostClient.Safe(() => OnConnect?.Invoke(Connect));
                    return;
                case "clock":
                    offset = MostJson.Long(v, "offset", offset);
                    rtt[MostTo.Host] = (int)MostJson.Long(v, "rtt");
                    MostClient.Safe(() => OnRtt?.Invoke());
                    return;
                case "rtt":
                    if (MostJson.Get(v, "rtt") is Dictionary<string, object> d)
                        foreach (var kv in d) if (ushort.TryParse(kv.Key, out var c) && c != MostTo.Host && c != Me) rtt[c] = (int)MostJson.ToLong(kv.Value);
                    MostClient.Safe(() => OnRtt?.Invoke());
                    return;
                case "ended":
                    if (!InSession) return;
                    Reset();
                    var why = MostJson.Str(v, "why");
                    MostClient.Safe(() => OnEnded?.Invoke(why));
                    return;
                case "sessions":
                {
                    var list = new List<MostSessionInfo>();
                    if (MostJson.Get(v, "list") is List<object> l) foreach (var x in l) list.Add(MostSessionInfo.From(x));
                    MostClient.Safe(() => OnSessions?.Invoke(list));
                    return;
                }
            }
        }

        internal void HandleClosed(string why)
        {
            if (!InSession) return;
            Reset();
            MostClient.Safe(() => OnEnded?.Invoke(why));
        }
    }
}
