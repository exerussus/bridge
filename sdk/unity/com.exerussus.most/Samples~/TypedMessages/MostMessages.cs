using System;
using System.Collections.Generic;
using System.Text;
using Exerussus.Most;
using UnityEngine;

/// <summary>
/// Один канал — свои типы сообщений. Первый байт — тип, дальше — тело в вашем формате (здесь JsonUtility;
/// для частых — свой BinaryWriter). Так же устроен «один бродкаст со своими пейлоудами» в сетевых библиотеках.
/// </summary>
public sealed class MostMessages
{
    readonly MostSession session;
    readonly Dictionary<byte, Action<ushort, ArraySegment<byte>>> handlers = new Dictionary<byte, Action<ushort, ArraySegment<byte>>>();

    public MostMessages(MostSession session)
    {
        this.session = session;
        session.OnData += Dispatch;
    }

    /// <summary>Принимать сообщения типа <paramref name="type"/> как T (JsonUtility).</summary>
    public void On<T>(byte type, Action<ushort, T> handler) =>
        handlers[type] = (from, body) => handler(from, JsonUtility.FromJson<T>(Encoding.UTF8.GetString(body.Array, body.Offset, body.Count)));

    /// <summary>Свой разбор тела (байты).</summary>
    public void OnRaw(byte type, Action<ushort, ArraySegment<byte>> handler) => handlers[type] = handler;

    public void Send<T>(ushort to, byte type, T msg, MostDelivery delivery = MostDelivery.Reliable, byte key = 0)
    {
        var body = Encoding.UTF8.GetBytes(JsonUtility.ToJson(msg));
        SendRaw(to, type, body, delivery, key);
    }

    public void SendRaw(ushort to, byte type, byte[] body, MostDelivery delivery = MostDelivery.Reliable, byte key = 0)
    {
        var f = new byte[body.Length + 1];
        f[0] = type;
        Buffer.BlockCopy(body, 0, f, 1, body.Length);
        session.Send(to, f, delivery, key);
    }

    void Dispatch(MostData d)
    {
        if (d.Length < 1 || !handlers.TryGetValue(d[0], out var h)) return;
        h(d.From, new ArraySegment<byte>(d.Payload.Array, d.Payload.Offset + 1, d.Payload.Count - 1));
    }
}

/// <summary>Пример: чат и ход игрока.</summary>
public sealed class MostMessagesDemo : MonoBehaviour
{
    [Serializable] public struct Chat { public string text; }
    [Serializable] public struct Move { public int x, y; }
    const byte ChatMsg = 1, MoveMsg = 2;
    MostMessages msgs;

    async void Start()
    {
        var c = Most.Connect();
        await c.WhenOpen;
        await c.Session.JoinOrCreate();
        msgs = new MostMessages(c.Session);
        msgs.On<Chat>(ChatMsg, (from, m) => Debug.Log("#" + from + ": " + m.text));
        msgs.On<Move>(MoveMsg, (from, m) => Debug.Log("#" + from + " ходит на " + m.x + "," + m.y));
        msgs.Send(MostTo.Others, ChatMsg, new Chat { text = "привет!" });
    }

    void Update()
    {
        if (msgs != null && Input.GetKeyDown(KeyCode.Space))
            msgs.Send(MostTo.Host, MoveMsg, new Move { x = UnityEngine.Random.Range(0, 8), y = UnityEngine.Random.Range(0, 8) }, MostDelivery.Latest, 2);
    }
}
