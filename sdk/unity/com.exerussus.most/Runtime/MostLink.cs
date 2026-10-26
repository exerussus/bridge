using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Net.WebSockets;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Exerussus.Most
{
    /// <summary>Входящее от моста: текст (JSON-событие), кадр данных или закрытие связи.</summary>
    internal struct MostIncoming
    {
        public string Text;
        public byte[] Bin;
        public string Closed;
    }

    /// <summary>Канал до моста: WebSocket (свои окна, редактор) или панель моста (WebGL в рамке).</summary>
    internal interface IMostLink
    {
        void Open(ConcurrentQueue<MostIncoming> inbox);
        void SendText(string json);
        void SendBinary(byte[] frame);
        /// <summary>Вызов вне сеанса: store.get, store.set, notify, toast, me, ticket. re — номер для ответа (WebGL).</summary>
        Task<object> Call(long re, string method, List<object> args);
        void Pump();
        void Close();
    }

    /// <summary>Свои окна и редактор: WebSocket к мосту на 127.0.0.1 по токену приложения.</summary>
    internal sealed class MostWsLink : IMostLink
    {
        readonly string api, token, name;
        readonly ClientWebSocket ws = new ClientWebSocket();
        readonly CancellationTokenSource cts = new CancellationTokenSource();
        readonly ConcurrentQueue<(bool bin, byte[] data)> outbox = new ConcurrentQueue<(bool, byte[])>();
        readonly SemaphoreSlim outSignal = new SemaphoreSlim(0);
        static readonly HttpClient http = new HttpClient { Timeout = TimeSpan.FromSeconds(20) };
        ConcurrentQueue<MostIncoming> inbox;
        int closed;

        /// <param name="api">MOST_API: http://127.0.0.1:порт/bridge/v1/app</param>
        public MostWsLink(string api, string token, string name)
        {
            this.api = api.TrimEnd('/');
            this.token = token;
            this.name = name ?? "";
        }

        public void Open(ConcurrentQueue<MostIncoming> inbox)
        {
            this.inbox = inbox;
            _ = Run();
        }

        async Task Run()
        {
            try
            {
                // сначала обычный запрос — у моста понятный ответ (неверный токен, не разрешено…)
                var me = await Http(HttpMethod.Get, "me", null).ConfigureAwait(false);
                if (me == null) throw new MostException("мост не ответил");
                var url = (api.StartsWith("https://") ? "wss://" + api.Substring(8) : "ws://" + api.Substring(api.IndexOf("://", StringComparison.Ordinal) + 3)) + "/net";
                if (name.Length > 0) url += "?name=" + Uri.EscapeDataString(name);
                ws.Options.SetRequestHeader("X-Most-App", token);
                ws.Options.KeepAliveInterval = TimeSpan.FromSeconds(15);
                await ws.ConnectAsync(new Uri(url), cts.Token).ConfigureAwait(false);
                _ = SendLoop();
                await ReceiveLoop().ConfigureAwait(false);
                Finish("мост закрыл связь");
            }
            catch (MostException e) { Finish(e.Message); }
            catch (Exception e) { Finish(closed == 0 ? "нет связи с мостом (" + api + "): " + e.Message : ""); }
        }

        async Task ReceiveLoop()
        {
            var buf = new byte[64 * 1024];
            var acc = new MemoryStream();
            while (ws.State == WebSocketState.Open && !cts.IsCancellationRequested)
            {
                var r = await ws.ReceiveAsync(new ArraySegment<byte>(buf), cts.Token).ConfigureAwait(false);
                if (r.MessageType == WebSocketMessageType.Close) return;
                acc.Write(buf, 0, r.Count);
                if (!r.EndOfMessage) continue;
                var data = acc.ToArray();
                acc.SetLength(0);
                if (r.MessageType == WebSocketMessageType.Text) inbox.Enqueue(new MostIncoming { Text = Encoding.UTF8.GetString(data) });
                else inbox.Enqueue(new MostIncoming { Bin = data });
            }
        }

        async Task SendLoop()
        {
            try
            {
                while (!cts.IsCancellationRequested)
                {
                    await outSignal.WaitAsync(cts.Token).ConfigureAwait(false);
                    while (outbox.TryDequeue(out var m))
                    {
                        if (ws.State != WebSocketState.Open) return;
                        await ws.SendAsync(new ArraySegment<byte>(m.data), m.bin ? WebSocketMessageType.Binary : WebSocketMessageType.Text, true, cts.Token).ConfigureAwait(false);
                    }
                }
            }
            catch (Exception) { /* закрытие — увидит ReceiveLoop */ }
        }

        void Finish(string why)
        {
            if (Interlocked.Exchange(ref closed, 1) != 0) return;
            inbox?.Enqueue(new MostIncoming { Closed = string.IsNullOrEmpty(why) ? "связь закрыта" : why });
            try { cts.Cancel(); } catch (Exception) { }
        }

        public void SendText(string json) { outbox.Enqueue((false, Encoding.UTF8.GetBytes(json))); outSignal.Release(); }
        public void SendBinary(byte[] frame) { outbox.Enqueue((true, frame)); outSignal.Release(); }

        public Task<object> Call(long re, string method, List<object> args)
        {
            object A(int i) => args != null && i < args.Count ? args[i] : null;
            switch (method)
            {
                case "me": return Http(HttpMethod.Get, "me", null);
                case "store.get": return Http(HttpMethod.Get, "store?key=" + Uri.EscapeDataString(A(0) as string ?? ""), null).ContinueWith(t => MostJson.Get(t.Result, "value"), TaskContinuationOptions.OnlyOnRanToCompletion);
                case "store.set": return Http(HttpMethod.Post, "store", new Dictionary<string, object> { ["key"] = A(0) as string ?? "", ["value"] = A(1) });
                case "notify": return Http(HttpMethod.Post, "notify", new Dictionary<string, object> { ["text"] = A(0) as string ?? "" });
                case "toast": return Http(HttpMethod.Post, "toast", new Dictionary<string, object> { ["text"] = A(0) as string ?? "" });
                case "ticket": return Http(HttpMethod.Get, "ticket?t=" + Uri.EscapeDataString(A(0) as string ?? ""), null);
                default: return Task.FromException<object>(new MostException("нет такого вызова: " + method));
            }
        }

        async Task<object> Http(HttpMethod m, string rel, object body)
        {
            using (var req = new HttpRequestMessage(m, api + "/" + rel))
            {
                req.Headers.Add("X-Most-App", token);
                if (body != null) req.Content = new StringContent(MostJson.Serialize(body), Encoding.UTF8, "application/json");
                using (var res = await http.SendAsync(req).ConfigureAwait(false))
                {
                    var text = await res.Content.ReadAsStringAsync().ConfigureAwait(false);
                    object v = null;
                    try { v = MostJson.Parse(text); } catch (Exception) { /* не JSON */ }
                    if (!res.IsSuccessStatusCode)
                    {
                        var err = MostJson.Str(v, "error");
                        throw new MostException(string.IsNullOrEmpty(err) ? "мост ответил " + (int)res.StatusCode : err);
                    }
                    return v;
                }
            }
        }

        public void Pump() { }

        public void Close()
        {
            try { if (ws.State == WebSocketState.Open) _ = ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "", CancellationToken.None).ContinueWith(_ => Finish("закрыто приложением")); else Finish("закрыто приложением"); }
            catch (Exception) { Finish("закрыто приложением"); }
        }
    }
}
