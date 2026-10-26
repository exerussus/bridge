using System.Collections.Generic;
using Exerussus.Most;
using UnityEngine;

/// <summary>
/// Курсоры всех, кто в вашем голосовом канале. Повесьте на любой объект сцены и запустите.
/// Курсор — «последнее» по ключу 1 (устаревшее не доставляется), щелчок — надёжно, счёт щелчков — общее состояние.
/// </summary>
public sealed class MostCursors : MonoBehaviour
{
    readonly Dictionary<ushort, Vector2> cursors = new Dictionary<ushort, Vector2>();
    readonly List<(Vector2 p, float t)> dots = new List<(Vector2, float)>();
    string status = "подключаюсь…";
    float nextSend;
    Vector2 lastSent;

    async void Start()
    {
        var c = Most.Connect();
        try
        {
            await c.WhenOpen;
            if (!c.Channel.InChannel) { status = "зайдите в голосовой канал в мосте"; c.OnChannel += ch => { if (!Most.Session.InSession && Most.Client.Channel.InChannel) _ = Join(); }; return; }
            await Join();
        }
        catch (MostException e) { status = e.Message; }
    }

    async System.Threading.Tasks.Task Join()
    {
        var s = Most.Session;
        s.OnData -= OnData; s.OnData += OnData;
        s.OnLeft -= OnLeft; s.OnLeft += OnLeft;
        try
        {
            await s.JoinOrCreate(new MostSessionOptions { State = new Dictionary<string, object> { ["clicks"] = 0L } });
            status = s.IsHost ? "вы хост" : "в сеансе";
        }
        catch (MostException e) { status = e.Message; }
    }

    void OnLeft(ushort cid, string why) => cursors.Remove(cid);

    void OnData(MostData d)
    {
        if (d.Length < 5) return;
        var p = new Vector2((d[1] | (d[2] << 8)) / 65535f, (d[3] | (d[4] << 8)) / 65535f);
        if (d[0] == 1) cursors[d.From] = p;
        else if (d[0] == 2) dots.Add((p, Time.time));
    }

    static byte[] Pack(byte kind, Vector2 p)
    {
        ushort x = (ushort)(Mathf.Clamp01(p.x) * 65535), y = (ushort)(Mathf.Clamp01(p.y) * 65535);
        return new[] { kind, (byte)x, (byte)(x >> 8), (byte)y, (byte)(y >> 8) };
    }

    void Update()
    {
        var s = Most.Session;
        if (s == null || !s.InSession) return;
        var p = new Vector2(Input.mousePosition.x / Screen.width, 1 - Input.mousePosition.y / Screen.height);
        if (Time.unscaledTime >= nextSend && p != lastSent)
        {
            nextSend = Time.unscaledTime + 1f / 30;
            lastSent = p;
            s.Send(MostTo.Others, Pack(1, p), MostDelivery.Latest, 1);
        }
        if (Input.GetMouseButtonDown(0))
        {
            dots.Add((p, Time.time));
            s.Send(MostTo.Others, Pack(2, p));
            _ = s.State.Set("clicks", s.State.GetLong("clicks") + 1);
        }
        dots.RemoveAll(x => Time.time - x.t > 3);
    }

    void OnGUI()
    {
        var s = Most.Session;
        GUI.Label(new Rect(10, 10, 600, 24), "Мост: " + status + (s != null && s.InSession ? " · игроков: " + s.Members.Count + " · щелчков: " + s.State.GetLong("clicks") + " · задержка " + s.Rtt() + " мс" : ""));
        foreach (var kv in cursors)
        {
            var m = s?.Member(kv.Key);
            var pos = new Vector2(kv.Value.x * Screen.width, kv.Value.y * Screen.height);
            GUI.Label(new Rect(pos.x, pos.y, 200, 22), "▲ " + (m != null ? m.Name : "#" + kv.Key));
        }
        foreach (var d in dots) GUI.Label(new Rect(d.p.x * Screen.width - 6, d.p.y * Screen.height - 10, 20, 20), "●");
    }
}
