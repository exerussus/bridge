using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;
using UnityEditor;
using UnityEditor.Build.Reporting;
using UnityEngine;

namespace Exerussus.Most.Editor
{
    /// <summary>
    /// Сборка приложения прямо в папку моста: своё окно (bin/…) или WebGL (ui/…), и app.json рядом.
    /// Мост сам замечает изменения; после новой сборки он попросит разрешить приложение заново (изменился код).
    /// </summary>
    public static class MostBuild
    {
        public static void Build(bool webgl)
        {
            var folder = MostEditorSettings.AppFolder;
            if (string.IsNullOrEmpty(folder))
            {
                EditorUtility.DisplayDialog("Мост", "Укажите папку приложения: «Мост → Подключение» → «Папка» (внутри папки «приложения» моста).", "Хорошо");
                MostWindow.Open();
                return;
            }
            Directory.CreateDirectory(folder);
            var scenes = EditorBuildSettings.scenes.Where(s => s.enabled).Select(s => s.path).ToArray();
            if (scenes.Length == 0)
            {
                EditorUtility.DisplayDialog("Мост", "В Build Settings нет включённых сцен.", "Хорошо");
                return;
            }
            var exeName = Safe(Application.productName);
            BuildTarget target;
            string location;
            if (webgl)
            {
                target = BuildTarget.WebGL;
                location = Path.Combine(folder, "ui");
            }
            else
            {
                target = Application.platform == RuntimePlatform.OSXEditor ? BuildTarget.StandaloneOSX
                    : Application.platform == RuntimePlatform.LinuxEditor ? BuildTarget.StandaloneLinux64
                    : BuildTarget.StandaloneWindows64;
                var ext = target == BuildTarget.StandaloneWindows64 ? ".exe" : target == BuildTarget.StandaloneOSX ? ".app" : "";
                location = Path.Combine(folder, "bin", exeName + ext);
            }

            // WebGL в мосте: рамка изолирована (без своего источника) — без кэша IndexedDB и без сжатия (мост не шлёт Content-Encoding)
            var oldCompression = PlayerSettings.WebGL.compressionFormat;
            var oldCaching = PlayerSettings.WebGL.dataCaching;
            if (webgl)
            {
                PlayerSettings.WebGL.compressionFormat = WebGLCompressionFormat.Disabled;
                PlayerSettings.WebGL.dataCaching = false;
            }
            BuildReport report;
            try
            {
                report = BuildPipeline.BuildPlayer(new BuildPlayerOptions { scenes = scenes, locationPathName = location, target = target, options = BuildOptions.None });
            }
            finally
            {
                if (webgl)
                {
                    PlayerSettings.WebGL.compressionFormat = oldCompression;
                    PlayerSettings.WebGL.dataCaching = oldCaching;
                }
            }
            if (report.summary.result != BuildResult.Succeeded)
            {
                Debug.LogError("[Мост] Сборка не удалась: " + report.summary.result);
                return;
            }

            var patch = new Dictionary<string, object> { ["version"] = string.IsNullOrEmpty(PlayerSettings.bundleVersion) ? "0.1.0" : PlayerSettings.bundleVersion };
            if (webgl)
            {
                AddSdk(Path.Combine(location, "index.html"));
                patch["ui"] = "ui/index.html";
            }
            else
            {
                var rel = "bin/" + exeName + (target == BuildTarget.StandaloneWindows64 ? ".exe" : target == BuildTarget.StandaloneOSX ? ".app/Contents/MacOS/" + exeName : "");
                patch["native"] = new Dictionary<string, object> { ["cmd"] = rel };
            }
            WriteManifest(folder, patch);
            Debug.Log("[Мост] Собрано в " + folder + " — откройте приложение в мосте (раздел «Приложения»).");
            EditorUtility.RevealInFinder(Path.Combine(folder, "app.json"));
        }

        /// <summary>Создать или дополнить app.json (известные поля из patch заменяются, остальное — как было).</summary>
        public static void WriteManifest(string folder, Dictionary<string, object> patch)
        {
            var path = Path.Combine(folder, "app.json");
            Dictionary<string, object> m = null;
            if (File.Exists(path))
            {
                try { m = MostJson.Parse(File.ReadAllText(path, Encoding.UTF8)) as Dictionary<string, object>; }
                catch (Exception e) { Debug.LogError("[Мост] app.json не читается, не трогаю: " + e.Message); return; }
            }
            if (m == null)
            {
                var id = string.IsNullOrEmpty(MostEditorSettings.AppId) ? Id(Application.productName) : MostEditorSettings.AppId;
                m = new Dictionary<string, object>
                {
                    ["id"] = id,
                    ["name"] = Application.productName,
                    ["version"] = string.IsNullOrEmpty(PlayerSettings.bundleVersion) ? "0.1.0" : PlayerSettings.bundleVersion,
                    ["description"] = "",
                    ["author"] = PlayerSettings.companyName,
                    ["window"] = new Dictionary<string, object> { ["w"] = 1280L, ["h"] = 720L },
                    ["net"] = new Dictionary<string, object> { ["proto"] = 1L, ["max"] = 8L },
                    ["permissions"] = new List<object> { "sessions" },
                    ["bridge"] = "apps@1",
                };
                MostEditorSettings.AppId = id;
            }
            if (patch != null) foreach (var kv in patch) m[kv.Key] = kv.Value;
            File.WriteAllText(path, MostJson.Pretty(m) + "\n", new UTF8Encoding(false));
        }

        /// <summary>WebGL: подключить SDK моста (window.most) в index.html.</summary>
        static void AddSdk(string index)
        {
            if (!File.Exists(index)) return;
            var html = File.ReadAllText(index, Encoding.UTF8);
            if (html.Contains("/most/sdk.js")) return;
            const string tag = "<script src=\"/most/sdk.js\"></script>";
            html = Regex.IsMatch(html, "</head>", RegexOptions.IgnoreCase) ? Regex.Replace(html, "</head>", tag + "\n</head>", RegexOptions.IgnoreCase) : tag + "\n" + html;
            File.WriteAllText(index, html, new UTF8Encoding(false));
        }

        static string Safe(string s)
        {
            var b = new StringBuilder();
            foreach (var c in s ?? "") b.Append(Path.GetInvalidFileNameChars().Contains(c) ? '_' : c);
            return b.Length == 0 ? "app" : b.ToString();
        }

        static string Id(string s)
        {
            var b = new StringBuilder();
            foreach (var c in (s ?? "").ToLowerInvariant())
                if ((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' || c == '_') b.Append(c);
                else if (c == ' ' && b.Length > 0 && b[b.Length - 1] != '-') b.Append('-');
            var r = b.ToString().Trim('-');
            if (r.Length > 40) r = r.Substring(0, 40);
            return r.Length == 0 ? "unity-app" : r;
        }
    }
}
