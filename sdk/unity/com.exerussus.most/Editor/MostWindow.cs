using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;
using UnityEditor;
using UnityEngine;

namespace Exerussus.Most.Editor
{
    /// <summary>«Мост → Подключение»: связать редактор с мостом этого ПК, имитация сети, сборка приложения.</summary>
    public sealed class MostWindow : EditorWindow
    {
        static readonly HttpClient http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
        string status = "", appStatus = "";
        bool busy;
        Vector2 scroll;

        [MenuItem("Мост/Подключение…", priority = 0)]
        public static void Open()
        {
            var w = GetWindow<MostWindow>("Мост");
            w.minSize = new Vector2(380, 420);
            w.ReadManifest();
        }

        [MenuItem("Мост/Собрать приложение (своё окно)", priority = 20)]
        static void BuildNative() => MostBuild.Build(false);

        [MenuItem("Мост/Собрать приложение (WebGL в мосте)", priority = 21)]
        static void BuildWeb() => MostBuild.Build(true);

        void OnEnable() => ReadManifest();

        void ReadManifest()
        {
            appStatus = "";
            var f = MostEditorSettings.AppFolder;
            if (string.IsNullOrEmpty(f)) return;
            var p = Path.Combine(f, "app.json");
            if (!File.Exists(p)) { appStatus = "В папке нет app.json — создайте его кнопкой ниже."; return; }
            try
            {
                var m = MostJson.Parse(File.ReadAllText(p, Encoding.UTF8));
                var id = MostJson.Str(m, "id");
                if (!string.IsNullOrEmpty(id)) MostEditorSettings.AppId = id;
                appStatus = "«" + MostJson.Str(m, "name", id) + "» " + MostJson.Str(m, "version") + (MostJson.Get(m, "net") == null ? " · нет net в app.json — сеансы недоступны" : "");
            }
            catch (Exception e) { appStatus = "app.json не читается: " + e.Message; }
        }

        void OnGUI()
        {
            scroll = EditorGUILayout.BeginScrollView(scroll);
            EditorGUILayout.LabelField("Мост " + Most.Version, EditorStyles.boldLabel);
            EditorGUILayout.HelpBox("Редактор подключается к мосту этого ПК как ваше приложение. Сеанс — с теми, кто в вашем голосовом канале. Хозяин ПК подтверждает подключение в панели моста.", MessageType.None);

            EditorGUILayout.Space(6);
            EditorGUILayout.LabelField("Приложение", EditorStyles.boldLabel);
            using (new EditorGUILayout.HorizontalScope())
            {
                var folder = EditorGUILayout.TextField(new GUIContent("Папка", "Папка приложения в «приложения» моста: там app.json, туда же кладётся сборка"), MostEditorSettings.AppFolder);
                if (folder != MostEditorSettings.AppFolder) { MostEditorSettings.AppFolder = folder; ReadManifest(); }
                if (GUILayout.Button("…", GUILayout.Width(28)))
                {
                    var picked = EditorUtility.OpenFolderPanel("Папка приложения (внутри «приложения» моста)", MostEditorSettings.AppFolder, "");
                    if (!string.IsNullOrEmpty(picked)) { MostEditorSettings.AppFolder = picked; ReadManifest(); GUI.FocusControl(null); }
                }
            }
            var id = EditorGUILayout.TextField(new GUIContent("ID", "id из app.json"), MostEditorSettings.AppId);
            if (id != MostEditorSettings.AppId) MostEditorSettings.AppId = id;
            if (!string.IsNullOrEmpty(appStatus)) EditorGUILayout.LabelField(appStatus, EditorStyles.miniLabel);
            if (!string.IsNullOrEmpty(MostEditorSettings.AppFolder) && !File.Exists(Path.Combine(MostEditorSettings.AppFolder, "app.json")))
                if (GUILayout.Button("Создать app.json (сетевое приложение)")) { MostBuild.WriteManifest(MostEditorSettings.AppFolder, null); ReadManifest(); }

            EditorGUILayout.Space(6);
            EditorGUILayout.LabelField("Подключение", EditorStyles.boldLabel);
            var url = EditorGUILayout.TextField(new GUIContent("Адрес моста", "Обычно http://127.0.0.1:5390 — первый адрес из listen в bridge.json"), MostEditorSettings.BridgeUrl);
            if (url != MostEditorSettings.BridgeUrl) MostEditorSettings.BridgeUrl = url;
            var name = EditorGUILayout.TextField(new GUIContent("Подпись", "Видна в сеансе после имени ПК"), MostEditorSettings.InstanceName);
            if (name != MostEditorSettings.InstanceName) MostEditorSettings.InstanceName = name;

            var has = !string.IsNullOrEmpty(MostEditorSettings.Token);
            var forApp = MostEditorSettings.TokenApp;
            EditorGUILayout.LabelField("Состояние", has ? "подключён как «" + forApp + "»" + (forApp != MostEditorSettings.AppId ? " (ID изменился — подключите заново)" : "") : "не подключён");
            using (new EditorGUI.DisabledScope(busy || string.IsNullOrEmpty(MostEditorSettings.AppId)))
            using (new EditorGUILayout.HorizontalScope())
            {
                if (GUILayout.Button(busy ? "Ждём ответа в панели моста…" : has ? "Подключить заново" : "Подключить редактор", GUILayout.Height(26))) _ = Pair();
                if (has && GUILayout.Button("Забыть", GUILayout.Width(70), GUILayout.Height(26))) { MostEditorSettings.Token = ""; status = "Токен забыт."; }
            }
            if (!string.IsNullOrEmpty(status)) EditorGUILayout.HelpBox(status, MessageType.None);

            EditorGUILayout.Space(6);
            EditorGUILayout.LabelField("Имитация сети (входящее, в режиме игры)", EditorStyles.boldLabel);
            MostEditorSettings.SimEnabled = EditorGUILayout.Toggle("Включить", MostEditorSettings.SimEnabled);
            using (new EditorGUI.DisabledScope(!MostEditorSettings.SimEnabled))
            {
                MostEditorSettings.SimLatency = EditorGUILayout.IntSlider("Задержка, мс", MostEditorSettings.SimLatency, 0, 1000);
                MostEditorSettings.SimJitter = EditorGUILayout.IntSlider("Разброс ±, мс", MostEditorSettings.SimJitter, 0, 300);
                MostEditorSettings.SimLoss = EditorGUILayout.Slider("Потери «быстро»", MostEditorSettings.SimLoss, 0f, 0.5f);
            }
            if (Application.isPlaying && Most.Client != null) Most.Client.Simulation = MostEditorSettings.Simulation;

            EditorGUILayout.Space(6);
            EditorGUILayout.LabelField("Сборка", EditorStyles.boldLabel);
            using (new EditorGUI.DisabledScope(string.IsNullOrEmpty(MostEditorSettings.AppFolder)))
            using (new EditorGUILayout.HorizontalScope())
            {
                if (GUILayout.Button("Своё окно")) EditorApplication.delayCall += () => MostBuild.Build(false);
                if (GUILayout.Button("WebGL в мосте")) EditorApplication.delayCall += () => MostBuild.Build(true);
            }
            EditorGUILayout.LabelField("Сборка кладётся в папку приложения и обновляет app.json. Мост заметит её сам; после изменения кода он попросит разрешить приложение заново.", EditorStyles.wordWrappedMiniLabel);

            if (Application.isPlaying && Most.Client != null)
            {
                EditorGUILayout.Space(6);
                EditorGUILayout.LabelField("Сейчас", EditorStyles.boldLabel);
                var c = Most.Client;
                EditorGUILayout.LabelField("Связь", c.State == MostClient.Status.Open ? "есть · " + c.NodeName : c.State == MostClient.Status.Connecting ? "подключаюсь…" : "закрыта: " + c.CloseReason);
                EditorGUILayout.LabelField("Канал", c.Channel.InChannel ? c.Channel.Title + " · " + string.Join(", ", c.Channel.Members) : "не в голосовом канале");
                var s = c.Session;
                EditorGUILayout.LabelField("Сеанс", s.InSession ? (s.IsHost ? "хост" : "участник #" + s.Me) + " · " + s.Members.Count + " из " + s.Max + " · задержка " + s.Rtt() + " мс" : "нет");
                Repaint();
            }
            EditorGUILayout.EndScrollView();
        }

        async Task Pair()
        {
            busy = true;
            status = "Запрос отправлен — подтвердите в панели моста (раздел «Приложения»).";
            Repaint();
            try
            {
                var app = MostEditorSettings.AppId;
                var who = "Unity · " + Application.productName + (string.IsNullOrEmpty(MostEditorSettings.InstanceName) ? "" : " · " + MostEditorSettings.InstanceName);
                var r = await Send(HttpMethod.Post, "dev", MostJson.Serialize(new Dictionary<string, object> { ["app"] = app, ["who"] = who }));
                var req = MostJson.Str(r, "req");
                for (var i = 0; i < 120; i++)
                {
                    await Task.Delay(1000);
                    var st = await Send(HttpMethod.Get, "dev?req=" + Uri.EscapeDataString(req), null);
                    var state = MostJson.Str(st, "state");
                    if (state == "ok")
                    {
                        MostEditorSettings.Token = MostJson.Str(st, "token");
                        MostEditorSettings.TokenApp = app;
                        status = "Готово: редактор подключён как «" + app + "». Токен действует до перезапуска моста.";
                        return;
                    }
                    if (state == "no") { status = "Хозяин ПК отказал в подключении."; return; }
                    if (state == "gone") { status = "Запрос устарел — попробуйте ещё раз."; return; }
                }
                status = "Ответа нет 2 минуты — попробуйте ещё раз.";
            }
            catch (Exception e) { status = "Не вышло: " + e.Message; }
            finally { busy = false; Repaint(); }
        }

        static async Task<object> Send(HttpMethod m, string rel, string body)
        {
            using (var req = new HttpRequestMessage(m, MostEditorSettings.Api + "/" + rel))
            {
                if (body != null) req.Content = new StringContent(body, Encoding.UTF8, "application/json");
                HttpResponseMessage res;
                try { res = await http.SendAsync(req); }
                catch (Exception e) { throw new MostException("мост не отвечает по адресу " + MostEditorSettings.BridgeUrl + " (" + e.Message + ")"); }
                using (res)
                {
                    var text = await res.Content.ReadAsStringAsync();
                    object v = null;
                    try { v = MostJson.Parse(text); } catch (Exception) { }
                    if (!res.IsSuccessStatusCode) throw new MostException(MostJson.Str(v, "error", "мост ответил " + (int)res.StatusCode));
                    return v;
                }
            }
        }
    }
}
