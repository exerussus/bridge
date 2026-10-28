using System;
using UnityEngine;

namespace Exerussus.Most
{
    /// <summary>Как подключаться.</summary>
    public sealed class MostOptions
    {
        /// <summary>Подпись экземпляра (видна в сеансе после имени ПК): «окно 2», имя игрока…</summary>
        public string Name;
        /// <summary>Имитация плохой сети. В редакторе по умолчанию — из окна «Мост → Подключение».</summary>
        public MostSimulation Simulation;
    }

    /// <summary>
    /// Точка входа. <code>var c = Most.Connect(); await c.WhenOpen; await c.Session.JoinOrCreate();</code>
    /// Откуда берётся доступ: запущено мостом — MOST_API и MOST_TOKEN из окружения; в редакторе — «Мост → Подключение»;
    /// WebGL в рамке моста — через панель (window.most).
    /// </summary>
    public static class Most
    {
        public const string Version = "3.7.0";

        /// <summary>Текущее подключение (или null).</summary>
        public static MostClient Client { get; private set; }
        public static MostSession Session => Client?.Session;

        /// <summary>Доступ редактора: (MOST_API, токен). Задаёт пакет редактора; в сборке не используется.</summary>
        public static Func<(string api, string token)> EditorCredentials;
        /// <summary>Имитация сети из настроек редактора.</summary>
        public static Func<MostSimulation> EditorSimulation;
        /// <summary>Подпись экземпляра из настроек редактора.</summary>
        public static Func<string> EditorName;
        /// <summary>Мост отверг токен редактора (мост перезапущен и т. п.).</summary>
        public static Action<string> EditorTokenRejected;

        /// <summary>Есть ли откуда подключиться (не гарантирует, что мост ответит).</summary>
        public static bool Available
        {
            get
            {
#if UNITY_WEBGL && !UNITY_EDITOR
                return MostWebLink.Available;
#else
                if (!string.IsNullOrEmpty(Env("MOST_TOKEN"))) return true;
                var c = EditorCredentials?.Invoke();
                return c.HasValue && !string.IsNullOrEmpty(c.Value.token);
#endif
            }
        }

        /// <summary>Подключиться (или вернуть уже открытое подключение). Ждите <see cref="MostClient.WhenOpen"/>.</summary>
        public static MostClient Connect(MostOptions options = null)
        {
            if (Client != null && Client.State != MostClient.Status.Closed) return Client;
            options ??= new MostOptions();
            IMostLink link;
#if UNITY_WEBGL && !UNITY_EDITOR
            link = new MostWebLink();
#else
            string api = Env("MOST_API"), token = Env("MOST_TOKEN"), why = "";
            var fromEditor = false;
            if (string.IsNullOrEmpty(token) && EditorCredentials != null)
            {
                var c = EditorCredentials();
                api = c.api;
                token = c.token;
                fromEditor = true;
                if (string.IsNullOrEmpty(token)) why = "редактор не подключён к мосту: «Мост → Подключение» → «Подключить редактор»";
            }
            else if (string.IsNullOrEmpty(token)) why = "приложение запущено не из моста (нет MOST_TOKEN). Положите сборку в папку «приложения» и откройте её в мосте";
            var name = options.Name ?? (fromEditor ? EditorName?.Invoke() : null);
            link = string.IsNullOrEmpty(why) ? new MostWsLink(api, token, name) : (IMostLink)new FailedLink(why);
#endif
            Client = new MostClient(link) { Simulation = options.Simulation ?? EditorSimulation?.Invoke() ?? new MostSimulation() };
#if !(UNITY_WEBGL && !UNITY_EDITOR)
            if (fromEditor)
            {
                var me = Client;
                me.OnClosed += why2 =>
                {
                    if (me.State == MostClient.Status.Closed && why2.Contains("токен")) EditorTokenRejected?.Invoke(why2);
                };
            }
#endif
            MostRunner.Ensure();
            return Client;
        }

        /// <summary>Закрыть подключение (сеанс тоже закончится).</summary>
        public static void Disconnect()
        {
            Client?.Close();
            Client = null;
        }

        /// <summary>Разобрать очередь входящего. Зовёт MostRunner каждый кадр; свой цикл — зовите сами.</summary>
        public static void Pump() => Client?.Pump();

        static string Env(string k)
        {
            try { return Environment.GetEnvironmentVariable(k) ?? ""; }
            catch (Exception) { return ""; }
        }

        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.SubsystemRegistration)]
        static void ResetStatics() => Client = null;

        sealed class FailedLink : IMostLink
        {
            readonly string why;
            public FailedLink(string why) { this.why = why; }
            public void Open(System.Collections.Concurrent.ConcurrentQueue<MostIncoming> inbox) => inbox.Enqueue(new MostIncoming { Closed = why });
            public void SendText(string json) { }
            public void SendBinary(byte[] frame) { }
            public System.Threading.Tasks.Task<object> Call(long re, string method, System.Collections.Generic.List<object> args) => System.Threading.Tasks.Task.FromException<object>(new MostException(why));
            public void Pump() { }
            public void Close() { }
        }
    }

    /// <summary>Невидимый объект: каждый кадр разбирает входящее (события — в главном потоке).</summary>
    [AddComponentMenu("")]
    public sealed class MostRunner : MonoBehaviour
    {
        static MostRunner instance;

        internal static void Ensure()
        {
            if (instance != null || !Application.isPlaying) return;
            var go = new GameObject("[Мост]") { hideFlags = HideFlags.HideInHierarchy | HideFlags.NotEditable };
            DontDestroyOnLoad(go);
            instance = go.AddComponent<MostRunner>();
        }

        void Update() => Most.Pump();

        void OnApplicationQuit() => Most.Disconnect();

        void OnDestroy()
        {
            if (instance == this) instance = null;
        }
    }
}
