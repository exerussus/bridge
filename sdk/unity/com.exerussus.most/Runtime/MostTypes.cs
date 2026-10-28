using System;
using System.Collections.Generic;
using System.Text;

namespace Exerussus.Most
{
    /// <summary>Как доставить сообщение.</summary>
    public enum MostDelivery : byte
    {
        /// <summary>Надёжно и по порядку (до 1 МБ). Для событий, команд, чата.</summary>
        Reliable = 0,
        /// <summary>Быстро: может потеряться при перегрузке (до 1200 байт). Для частых мелочей.</summary>
        Fast = 1,
        /// <summary>Последнее по ключу (0–255): устаревшее не доставляется. Для позиций, курсоров.</summary>
        Latest = 2,
    }

    /// <summary>Адресаты: хост, конкретный участник (cid) или все.</summary>
    public static class MostTo
    {
        public const ushort Host = 0;
        public const ushort All = 0xFFFF;
        public const ushort Others = 0xFFFE;
    }

    /// <summary>Участник сеанса.</summary>
    [Serializable]
    public sealed class MostMember
    {
        /// <summary>Номер в сеансе: 0 — хост.</summary>
        public ushort Cid;
        /// <summary>Имя моста (ПК), с которого участник.</summary>
        public string Node;
        /// <summary>Подпись: имя ПК и, если задано, имя экземпляра.</summary>
        public string Name;
        /// <summary>Ключ участника для издателя этого приложения (base64url), если у приложения есть право identity.
        /// Со слов его моста: надёжен только ключ, подтверждённый входом на вашем сервере.</summary>
        public string Key;
        public bool IsHost => Cid == MostTo.Host;

        internal static MostMember From(object o) => new MostMember
        {
            Cid = (ushort)MostJson.Long(o, "cid"),
            Node = MostJson.Str(o, "node"),
            Name = MostJson.Str(o, "name"),
            Key = MostJson.Str(o, "key"),
        };

        public override string ToString() => Name + " #" + Cid;
    }

    /// <summary>Сеанс, который идёт в вашем канале.</summary>
    [Serializable]
    public sealed class MostSessionInfo
    {
        public string Sid, App, Room, Title, Host, Transport;
        public int Count, Max;
        public bool Joinable;
        public long Created;
        public List<string> Members = new List<string>();
        public object Meta;

        internal static MostSessionInfo From(object o)
        {
            var s = new MostSessionInfo
            {
                Sid = MostJson.Str(o, "sid"), App = MostJson.Str(o, "app"), Room = MostJson.Str(o, "room"), Title = MostJson.Str(o, "title"),
                Host = MostJson.Str(o, "host"), Transport = MostJson.Str(o, "transport", "bridge"),
                Count = (int)MostJson.Long(o, "count"), Max = (int)MostJson.Long(o, "max"), Joinable = MostJson.Bool(o, "joinable"),
                Created = MostJson.Long(o, "created"), Meta = MostJson.Get(o, "meta"),
            };
            if (MostJson.Get(o, "members") is List<object> l)
                foreach (var m in l) s.Members.Add(m as string ?? "");
            return s;
        }
    }

    /// <summary>Параметры нового сеанса.</summary>
    public sealed class MostSessionOptions
    {
        /// <summary>Сколько участников (по умолчанию — net.max из app.json).</summary>
        public int Max;
        /// <summary>Что показать в списке сеансов (карта, режим…).</summary>
        public object Meta;
        /// <summary>Начальное общее состояние.</summary>
        public object State;
        /// <summary>"bridge" (через мосты, по умолчанию) или "external" (свой сервер: строка подключения — SetConnect).</summary>
        public string Transport;

        internal Dictionary<string, object> Fill(Dictionary<string, object> d)
        {
            if (Max > 0) d["max"] = (long)Max;
            if (Meta != null) d["meta"] = MostJson.ToTree(Meta);
            if (State != null) d["state"] = MostJson.ToTree(State);
            if (!string.IsNullOrEmpty(Transport)) d["transport"] = Transport;
            return d;
        }
    }

    /// <summary>Голосовой канал, в котором сейчас хозяин ПК.</summary>
    [Serializable]
    public sealed class MostChannel
    {
        public string Room, Title;
        public List<string> Members = new List<string>();
        public bool InChannel => !string.IsNullOrEmpty(Room);

        internal static MostChannel From(object o)
        {
            var c = new MostChannel { Room = MostJson.Str(o, "room"), Title = MostJson.Str(o, "title") };
            if (MostJson.Get(o, "members") is List<object> l)
                foreach (var m in l) c.Members.Add(m as string ?? "");
            return c;
        }
    }

    /// <summary>Входящее сообщение. Буфер действителен только внутри обработчика — копируйте, если храните.</summary>
    public readonly struct MostData
    {
        public readonly ushort From;
        public readonly MostDelivery Delivery;
        public readonly ArraySegment<byte> Payload;

        public MostData(ushort from, MostDelivery delivery, ArraySegment<byte> payload)
        {
            From = from;
            Delivery = delivery;
            Payload = payload;
        }

        public int Length => Payload.Count;
        public byte this[int i] => Payload.Array[Payload.Offset + i];
        public byte[] ToArray() { var b = new byte[Payload.Count]; Buffer.BlockCopy(Payload.Array, Payload.Offset, b, 0, b.Length); return b; }
        public string Text() => Encoding.UTF8.GetString(Payload.Array, Payload.Offset, Payload.Count);
        public T Json<T>() => UnityEngine.JsonUtility.FromJson<T>(Text());
    }

    /// <summary>Ключ пользователя для издателя приложения (Identity.Get).</summary>
    [Serializable]
    public sealed class MostIdentityInfo
    {
        /// <summary>Открытый ключ Ed25519, base64url (43 знака). ID игрока у вас: например, most:&lt;Key&gt;.</summary>
        public string Key;
        /// <summary>Отпечаток для людей: a1b2-c3d4-e5f6-0718.</summary>
        public string Fingerprint;
        public string Name;
        /// <summary>Ключ издателя, если подпись пакета проверена; иначе пусто (приложение из папки).</summary>
        public string Publisher;
        public bool Verified;
        /// <summary>Переходы со старых ключей: строки, подписанные прежним ключом (purpose=revoke).</summary>
        public List<MostSignature> Rotated = new List<MostSignature>();

        internal static MostIdentityInfo From(object o)
        {
            var r = new MostIdentityInfo { Key = MostJson.Str(o, "key"), Fingerprint = MostJson.Str(o, "fingerprint"), Name = MostJson.Str(o, "name"), Publisher = MostJson.Str(o, "publisher"), Verified = MostJson.Bool(o, "verified") };
            if (MostJson.Get(o, "rotated") is List<object> l) foreach (var x in l) r.Rotated.Add(MostSignature.From(x));
            return r;
        }
    }

    /// <summary>Подпись (Identity.Sign): Signed — ровно подписанная строка, её и проверяйте на сервере.</summary>
    [Serializable]
    public sealed class MostSignature
    {
        public string Key, Sig, Signed;
        internal static MostSignature From(object o) => new MostSignature { Key = MostJson.Str(o, "key"), Sig = MostJson.Str(o, "sig"), Signed = MostJson.Str(o, "signed") };
    }

    /// <summary>Ошибка от моста (нет права, сеанс полон, не в канале…). Текст — для человека.</summary>
    public sealed class MostException : Exception
    {
        public MostException(string message) : base(message) { }
    }

    /// <summary>Имитация плохой сети для проверки игры (только входящее: задержка, разброс, потери «быстрого»).</summary>
    [Serializable]
    public sealed class MostSimulation
    {
        public bool Enabled;
        /// <summary>Задержка, мс.</summary>
        public int LatencyMs = 60;
        /// <summary>Разброс задержки ±, мс.</summary>
        public int JitterMs = 20;
        /// <summary>Доля потерь «быстро» и «последнее», 0–1 (надёжное не теряется).</summary>
        public float Loss = 0.02f;
    }
}
