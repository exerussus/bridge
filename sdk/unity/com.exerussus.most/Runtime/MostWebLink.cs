using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading.Tasks;

namespace Exerussus.Most
{
    /// <summary>
    /// WebGL-сборка в мосте: игра живёт в изолированной рамке, сеть и вызовы идут через панель моста (window.most).
    /// Сборку кладите в ui/ приложения: «Мост → Собрать приложение» с целью WebGL.
    /// </summary>
    internal sealed class MostWebLink : IMostLink
    {
#if UNITY_WEBGL && !UNITY_EDITOR
        [DllImport("__Internal")] static extern int MostWeb_Available();
        [DllImport("__Internal")] static extern void MostWeb_Open();
        [DllImport("__Internal")] static extern void MostWeb_SendText(string json);
        [DllImport("__Internal")] static extern void MostWeb_SendBin(byte[] data, int len);
        [DllImport("__Internal")] static extern void MostWeb_Call(int re, string method, string argsJson);
        [DllImport("__Internal")] static extern string MostWeb_NextText();
        [DllImport("__Internal")] static extern int MostWeb_Peek();
        [DllImport("__Internal")] static extern void MostWeb_NextBin(byte[] buf, int cap);
        [DllImport("__Internal")] static extern void MostWeb_Close();
        public static bool Available => MostWeb_Available() != 0;
#else
        static int MostWeb_Available() => 0;
        static void MostWeb_Open() { }
        static void MostWeb_SendText(string json) { }
        static void MostWeb_SendBin(byte[] data, int len) { }
        static void MostWeb_Call(int re, string method, string argsJson) { }
        static string MostWeb_NextText() => null;
        static int MostWeb_Peek() => -1;
        static void MostWeb_NextBin(byte[] buf, int cap) { }
        static void MostWeb_Close() { }
        public static bool Available => false;
#endif
        ConcurrentQueue<MostIncoming> inbox;

        public void Open(ConcurrentQueue<MostIncoming> inbox)
        {
            this.inbox = inbox;
            if (MostWeb_Available() == 0) { inbox.Enqueue(new MostIncoming { Closed = "WebGL-сборка открыта вне моста (нет window.most)" }); return; }
            MostWeb_Open();
        }

        public void SendText(string json) => MostWeb_SendText(json);
        public void SendBinary(byte[] frame) => MostWeb_SendBin(frame, frame.Length);

        public Task<object> Call(long re, string method, List<object> args)
        {
            MostWeb_Call((int)re, method, MostJson.Serialize(args ?? new List<object>()));
            return null; // ответ придёт событием {ev:"re"}
        }

        public void Pump()
        {
            if (inbox == null) return;
            // одна очередь в порядке прихода: -1 пусто, -2 текст, n ≥ 0 — кадр данных длиной n
            for (var i = 0; i < 8192; i++)
            {
                var n = MostWeb_Peek();
                if (n == -1) break;
                if (n == -2)
                {
                    var t = MostWeb_NextText() ?? "";
                    if (t.StartsWith("!closed:")) inbox.Enqueue(new MostIncoming { Closed = t.Substring(8) });
                    else inbox.Enqueue(new MostIncoming { Text = t });
                    continue;
                }
                var b = new byte[n];
                MostWeb_NextBin(b, n);
                inbox.Enqueue(new MostIncoming { Bin = b });
            }
        }

        public void Close() => MostWeb_Close();
    }
}
