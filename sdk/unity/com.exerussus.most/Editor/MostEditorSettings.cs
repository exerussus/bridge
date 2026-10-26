using System;
using UnityEditor;
using UnityEngine;

namespace Exerussus.Most.Editor
{
    /// <summary>
    /// Настройки редактора — в EditorPrefs этого ПК (не в проекте): токен моста не попадает в репозиторий.
    /// </summary>
    public static class MostEditorSettings
    {
        static string P => "Exerussus.Most." + Hash(Application.dataPath) + ".";

        public static string BridgeUrl { get => EditorPrefs.GetString(P + "url", "http://127.0.0.1:5390"); set => EditorPrefs.SetString(P + "url", (value ?? "").Trim().TrimEnd('/')); }
        public static string AppId { get => EditorPrefs.GetString(P + "app", ""); set => EditorPrefs.SetString(P + "app", (value ?? "").Trim()); }
        /// <summary>Папка приложения внутри «приложения» моста (там app.json; туда кладётся сборка).</summary>
        public static string AppFolder { get => EditorPrefs.GetString(P + "folder", ""); set => EditorPrefs.SetString(P + "folder", value ?? ""); }
        public static string Token { get => EditorPrefs.GetString(P + "token", ""); set => EditorPrefs.SetString(P + "token", value ?? ""); }
        /// <summary>Для какого приложения выдан токен.</summary>
        public static string TokenApp { get => EditorPrefs.GetString(P + "tokenApp", ""); set => EditorPrefs.SetString(P + "tokenApp", value ?? ""); }
        public static string InstanceName { get => EditorPrefs.GetString(P + "name", "редактор"); set => EditorPrefs.SetString(P + "name", value ?? ""); }

        public static bool SimEnabled { get => EditorPrefs.GetBool(P + "sim", false); set => EditorPrefs.SetBool(P + "sim", value); }
        public static int SimLatency { get => EditorPrefs.GetInt(P + "simLat", 60); set => EditorPrefs.SetInt(P + "simLat", value); }
        public static int SimJitter { get => EditorPrefs.GetInt(P + "simJit", 20); set => EditorPrefs.SetInt(P + "simJit", value); }
        public static float SimLoss { get => EditorPrefs.GetFloat(P + "simLoss", 0.02f); set => EditorPrefs.SetFloat(P + "simLoss", value); }

        public static string Api => BridgeUrl + "/bridge/v1/app";

        public static MostSimulation Simulation => new MostSimulation { Enabled = SimEnabled, LatencyMs = SimLatency, JitterMs = SimJitter, Loss = SimLoss };

        static string Hash(string s)
        {
            unchecked
            {
                uint h = 2166136261;
                foreach (var c in s) h = (h ^ c) * 16777619;
                return h.ToString("x8");
            }
        }
    }

    /// <summary>Отдаёт доступ редактора в рантайм (Most.Connect в режиме игры).</summary>
    [InitializeOnLoad]
    static class MostEditorHook
    {
        static MostEditorHook()
        {
            Most.EditorCredentials = () => (MostEditorSettings.Api, MostEditorSettings.Token);
            Most.EditorSimulation = () => MostEditorSettings.Simulation;
            Most.EditorName = () => MostEditorSettings.InstanceName;
            Most.EditorTokenRejected = why =>
            {
                MostEditorSettings.Token = "";
                Debug.LogWarning("[Мост] Токен редактора больше не действует (мост перезапущен или приложение изменилось): " + why + "\nОткройте «Мост → Подключение» и нажмите «Подключить редактор».");
            };
        }
    }
}
