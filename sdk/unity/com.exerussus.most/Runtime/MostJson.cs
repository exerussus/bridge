using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace Exerussus.Most
{
    /// <summary>
    /// Маленький JSON без зависимостей. Объекты — Dictionary&lt;string, object&gt;, массивы — List&lt;object&gt;,
    /// числа — long (целые) или double, строки, bool, null.
    /// </summary>
    public static class MostJson
    {
        /// <summary>Готовый JSON, который вставляется как есть (например, результат JsonUtility.ToJson).</summary>
        public readonly struct Raw
        {
            public readonly string Json;
            public Raw(string json) { Json = json; }
        }

        public static object Parse(string text)
        {
            if (string.IsNullOrEmpty(text)) return null;
            var p = new Parser(text);
            p.Ws();
            var v = p.Value();
            p.Ws();
            if (p.I < text.Length) throw new FormatException("JSON: лишнее после значения, позиция " + p.I);
            return v;
        }

        public static string Serialize(object value)
        {
            var sb = new StringBuilder(64);
            Write(sb, value);
            return sb.ToString();
        }

        /// <summary>С отступами — для файлов, которые правят руками (app.json).</summary>
        public static string Pretty(object value)
        {
            var sb = new StringBuilder(256);
            WritePretty(sb, ToTree(value), 0);
            return sb.ToString();
        }

        static void WritePretty(StringBuilder sb, object v, int depth)
        {
            var pad = new string(' ', (depth + 1) * 2);
            if (v is Dictionary<string, object> d && d.Count > 0)
            {
                sb.Append("{\n");
                var i = 0;
                foreach (var kv in d)
                {
                    sb.Append(pad);
                    Quote(sb, kv.Key);
                    sb.Append(": ");
                    WritePretty(sb, kv.Value, depth + 1);
                    if (++i < d.Count) sb.Append(',');
                    sb.Append('\n');
                }
                sb.Append(' ', depth * 2).Append('}');
                return;
            }
            if (v is List<object> l && l.Count > 0)
            {
                var simple = l.TrueForAll(x => !(x is Dictionary<string, object>) && !(x is List<object>));
                if (simple) { Write(sb, l); return; }
                sb.Append("[\n");
                for (var i = 0; i < l.Count; i++)
                {
                    sb.Append(pad);
                    WritePretty(sb, l[i], depth + 1);
                    if (i + 1 < l.Count) sb.Append(',');
                    sb.Append('\n');
                }
                sb.Append(' ', depth * 2).Append(']');
                return;
            }
            Write(sb, v);
        }

        /// <summary>Значение по пути «a.b.c» внутри объектов (или null).</summary>
        public static object Path(object root, string path)
        {
            if (string.IsNullOrEmpty(path)) return root;
            var cur = root;
            foreach (var k in path.Split('.'))
            {
                if (cur is Dictionary<string, object> d) { if (!d.TryGetValue(k, out cur)) return null; }
                else if (cur is List<object> l && int.TryParse(k, out var i)) { if (i < 0 || i >= l.Count) return null; cur = l[i]; }
                else return null;
            }
            return cur;
        }

        public static string Str(object o, string key, string def = "") => o is Dictionary<string, object> d && d.TryGetValue(key, out var v) && v != null ? (v as string ?? Convert.ToString(v, CultureInfo.InvariantCulture)) : def;
        public static long Long(object o, string key, long def = 0) => o is Dictionary<string, object> d && d.TryGetValue(key, out var v) ? ToLong(v, def) : def;
        public static bool Bool(object o, string key, bool def = false) => o is Dictionary<string, object> d && d.TryGetValue(key, out var v) && v is bool b ? b : def;
        public static object Get(object o, string key) => o is Dictionary<string, object> d && d.TryGetValue(key, out var v) ? v : null;

        public static long ToLong(object v, long def = 0)
        {
            switch (v)
            {
                case long l: return l;
                case double x: return (long)x;
                case int i: return i;
                case string s when long.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out var r): return r;
                default: return def;
            }
        }

        public static double ToDouble(object v, double def = 0)
        {
            switch (v)
            {
                case long l: return l;
                case double x: return x;
                case int i: return i;
                case float f: return f;
                case string s when double.TryParse(s, NumberStyles.Float, CultureInfo.InvariantCulture, out var r): return r;
                default: return def;
            }
        }

        /// <summary>Глубокая копия (объекты и массивы).</summary>
        public static object Clone(object v)
        {
            if (v is Dictionary<string, object> d)
            {
                var r = new Dictionary<string, object>(d.Count);
                foreach (var kv in d) r[kv.Key] = Clone(kv.Value);
                return r;
            }
            if (v is List<object> l)
            {
                var r = new List<object>(l.Count);
                foreach (var x in l) r.Add(Clone(x));
                return r;
            }
            return v;
        }

        /// <summary>Перевести любое значение в дерево MostJson (свои классы — через JsonUtility).</summary>
        public static object ToTree(object v)
        {
            if (v == null || v is string || v is bool || v is long || v is double || v is Dictionary<string, object> || v is List<object>) return v;
            if (v is int || v is short || v is byte || v is uint || v is ushort || v is sbyte) return Convert.ToInt64(v, CultureInfo.InvariantCulture);
            if (v is ulong ul) return (long)ul;
            if (v is float f) return (double)f;
            if (v is decimal m) return (double)m;
            if (v is Raw raw) return Parse(raw.Json);
            return Parse(Serialize(v));
        }

        static void Write(StringBuilder sb, object v)
        {
            switch (v)
            {
                case null: sb.Append("null"); return;
                case string s: Quote(sb, s); return;
                case bool b: sb.Append(b ? "true" : "false"); return;
                case Raw r: sb.Append(string.IsNullOrEmpty(r.Json) ? "null" : r.Json); return;
                case double d: WriteDouble(sb, d); return;
                case float f: WriteDouble(sb, f); return;
                case decimal m: sb.Append(m.ToString(CultureInfo.InvariantCulture)); return;
                case long or int or short or byte or sbyte or uint or ushort or ulong: sb.Append(Convert.ToString(v, CultureInfo.InvariantCulture)); return;
                case Enum e: sb.Append(Convert.ToInt64(e, CultureInfo.InvariantCulture).ToString(CultureInfo.InvariantCulture)); return;
                case IDictionary dict:
                {
                    sb.Append('{');
                    var first = true;
                    foreach (DictionaryEntry kv in dict)
                    {
                        if (!first) sb.Append(',');
                        first = false;
                        Quote(sb, Convert.ToString(kv.Key, CultureInfo.InvariantCulture));
                        sb.Append(':');
                        Write(sb, kv.Value);
                    }
                    sb.Append('}');
                    return;
                }
                case byte[] bytes: Quote(sb, Convert.ToBase64String(bytes)); return;
                case IEnumerable list:
                {
                    sb.Append('[');
                    var first = true;
                    foreach (var x in list)
                    {
                        if (!first) sb.Append(',');
                        first = false;
                        Write(sb, x);
                    }
                    sb.Append(']');
                    return;
                }
                default:
                    // свой [Serializable] класс или структура
                    sb.Append(UnityEngine.JsonUtility.ToJson(v));
                    return;
            }
        }

        static void WriteDouble(StringBuilder sb, double d)
        {
            if (double.IsNaN(d) || double.IsInfinity(d)) { sb.Append("null"); return; }
            if (Math.Abs(d % 1) < double.Epsilon && Math.Abs(d) < 9e15) sb.Append(((long)d).ToString(CultureInfo.InvariantCulture));
            else sb.Append(d.ToString("R", CultureInfo.InvariantCulture));
        }

        static void Quote(StringBuilder sb, string s)
        {
            sb.Append('"');
            foreach (var c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    case '\b': sb.Append("\\b"); break;
                    case '\f': sb.Append("\\f"); break;
                    default:
                        if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
        }

        sealed class Parser
        {
            readonly string s;
            public int I;
            public Parser(string s) { this.s = s; }

            public void Ws() { while (I < s.Length && (s[I] == ' ' || s[I] == '\t' || s[I] == '\n' || s[I] == '\r')) I++; }

            Exception Err(string what) => new FormatException("JSON: " + what + ", позиция " + I);

            public object Value()
            {
                if (I >= s.Length) throw Err("неожиданный конец");
                var c = s[I];
                switch (c)
                {
                    case '{': return Obj();
                    case '[': return Arr();
                    case '"': return Str();
                    case 't': Lit("true"); return true;
                    case 'f': Lit("false"); return false;
                    case 'n': Lit("null"); return null;
                    default:
                        if (c == '-' || (c >= '0' && c <= '9')) return Num();
                        throw Err("неожиданный символ «" + c + "»");
                }
            }

            void Lit(string w)
            {
                if (string.CompareOrdinal(s, I, w, 0, w.Length) != 0) throw Err("ожидалось " + w);
                I += w.Length;
            }

            Dictionary<string, object> Obj()
            {
                var d = new Dictionary<string, object>();
                I++;
                Ws();
                if (I < s.Length && s[I] == '}') { I++; return d; }
                while (true)
                {
                    Ws();
                    if (I >= s.Length || s[I] != '"') throw Err("ожидался ключ");
                    var k = Str();
                    Ws();
                    if (I >= s.Length || s[I] != ':') throw Err("ожидалось «:»");
                    I++;
                    Ws();
                    d[k] = Value();
                    Ws();
                    if (I >= s.Length) throw Err("неожиданный конец");
                    if (s[I] == ',') { I++; continue; }
                    if (s[I] == '}') { I++; return d; }
                    throw Err("ожидалось «,» или «}»");
                }
            }

            List<object> Arr()
            {
                var l = new List<object>();
                I++;
                Ws();
                if (I < s.Length && s[I] == ']') { I++; return l; }
                while (true)
                {
                    Ws();
                    l.Add(Value());
                    Ws();
                    if (I >= s.Length) throw Err("неожиданный конец");
                    if (s[I] == ',') { I++; continue; }
                    if (s[I] == ']') { I++; return l; }
                    throw Err("ожидалось «,» или «]»");
                }
            }

            string Str()
            {
                I++;
                StringBuilder sb = null;
                var start = I;
                while (I < s.Length)
                {
                    var c = s[I];
                    if (c == '"')
                    {
                        string r = sb == null ? s.Substring(start, I - start) : sb.Append(s, start, I - start).ToString();
                        I++;
                        return r;
                    }
                    if (c == '\\')
                    {
                        sb ??= new StringBuilder();
                        sb.Append(s, start, I - start);
                        I++;
                        if (I >= s.Length) break;
                        var e = s[I];
                        switch (e)
                        {
                            case '"': sb.Append('"'); break;
                            case '\\': sb.Append('\\'); break;
                            case '/': sb.Append('/'); break;
                            case 'b': sb.Append('\b'); break;
                            case 'f': sb.Append('\f'); break;
                            case 'n': sb.Append('\n'); break;
                            case 'r': sb.Append('\r'); break;
                            case 't': sb.Append('\t'); break;
                            case 'u':
                                if (I + 4 >= s.Length) throw Err("обрыв \\u");
                                sb.Append((char)int.Parse(s.Substring(I + 1, 4), NumberStyles.HexNumber, CultureInfo.InvariantCulture));
                                I += 4;
                                break;
                            default: throw Err("неизвестная замена \\" + e);
                        }
                        I++;
                        start = I;
                        continue;
                    }
                    I++;
                }
                throw Err("незакрытая строка");
            }

            object Num()
            {
                var start = I;
                if (s[I] == '-') I++;
                var frac = false;
                while (I < s.Length)
                {
                    var c = s[I];
                    if (c >= '0' && c <= '9') { I++; continue; }
                    if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') { frac = true; I++; continue; }
                    break;
                }
                var t = s.Substring(start, I - start);
                if (!frac && long.TryParse(t, NumberStyles.Integer, CultureInfo.InvariantCulture, out var l)) return l;
                if (double.TryParse(t, NumberStyles.Float, CultureInfo.InvariantCulture, out var d)) return d;
                throw Err("число «" + t + "»");
            }
        }
    }
}
