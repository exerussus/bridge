using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace Exerussus.Most
{
    /// <summary>
    /// Общее состояние сеанса: JSON-дерево у хоста, у всех — копия. Меняется операциями set / merge / del по пути «a.b.c»;
    /// хост применяет их по порядку и раздаёт всем (вместе с номером правки Rev). До 4 МБ.
    /// Для быстрых и частых данных (позиции) — не состояние, а Send(..., Latest).
    /// </summary>
    public sealed class MostState
    {
        readonly MostSession session;
        Dictionary<string, object> root = new Dictionary<string, object>();

        /// <summary>Номер последней правки.</summary>
        public long Rev { get; private set; }
        /// <summary>Всё дерево (только читать).</summary>
        public Dictionary<string, object> Root => root;

        /// <summary>Состояние изменилось: операции, кто изменил (cid).</summary>
        public event Action<IReadOnlyList<object>, ushort> OnChanged;

        internal MostState(MostSession s) { session = s; }

        public object Get(string path) => MostJson.Path(root, path);
        public long GetLong(string path, long def = 0) { var v = Get(path); return v == null ? def : MostJson.ToLong(v, def); }
        public double GetDouble(string path, double def = 0) { var v = Get(path); return v == null ? def : MostJson.ToDouble(v, def); }
        public string GetString(string path, string def = "") => Get(path) is string s ? s : def;
        public bool GetBool(string path, bool def = false) => Get(path) is bool b ? b : def;
        /// <summary>Узел как свой [Serializable] класс (через JsonUtility).</summary>
        public T Get<T>(string path) { var v = Get(path); return v == null ? default : UnityEngine.JsonUtility.FromJson<T>(MostJson.Serialize(v)); }

        public Task Set(string path, object value) => Apply(Op("set", path, value));
        /// <summary>Слить объект: поля со значением null удаляются.</summary>
        public Task Merge(string path, object value) => Apply(Op("merge", path, value));
        public Task Delete(string path) => Apply(Op("del", path, null));

        /// <summary>Несколько операций разом (одной правкой).</summary>
        public Task Apply(params Dictionary<string, object>[] ops)
        {
            var list = new List<object>(ops.Length);
            foreach (var o in ops) list.Add(o);
            return session.Client.Request("state", new Dictionary<string, object> { ["ops"] = list });
        }

        public static Dictionary<string, object> Op(string op, string path, object value)
        {
            var d = new Dictionary<string, object> { ["op"] = op, ["path"] = path ?? "" };
            if (op != "del") d["value"] = MostJson.ToTree(value);
            return d;
        }

        internal void Reset(object state, long rev)
        {
            root = state as Dictionary<string, object> ?? new Dictionary<string, object>();
            Rev = rev;
        }

        internal void Changed(object ev)
        {
            var ops = MostJson.Get(ev, "ops") as List<object> ?? new List<object>();
            foreach (var o in ops) ApplyOp(ref root, o);
            Rev = MostJson.Long(ev, "rev", Rev);
            OnChanged?.Invoke(ops, (ushort)MostJson.Long(ev, "by"));
        }

        /// <summary>Та же логика, что у хоста (node/src/net.rs apply_op).</summary>
        public static void ApplyOp(ref Dictionary<string, object> state, object o)
        {
            var op = MostJson.Str(o, "op");
            var pv = MostJson.Get(o, "path");
            var path = new List<string>();
            if (pv is List<object> pl) foreach (var x in pl) path.Add(x as string ?? MostJson.Serialize(x));
            else if (pv is string ps && ps.Length > 0) path.AddRange(ps.Split('.'));
            var value = MostJson.Get(o, "value");
            state ??= new Dictionary<string, object>();
            if (path.Count == 0)
            {
                if (op == "set") state = MostJson.Clone(value) as Dictionary<string, object> ?? new Dictionary<string, object>();
                else if (op == "merge") { object s = state; Merge(ref s, value); state = s as Dictionary<string, object> ?? new Dictionary<string, object>(); }
                return;
            }
            var cur = state;
            for (var i = 0; i < path.Count - 1; i++)
            {
                if (!(cur.TryGetValue(path[i], out var next) && next is Dictionary<string, object> nd))
                {
                    nd = new Dictionary<string, object>();
                    cur[path[i]] = nd;
                }
                cur = nd;
            }
            var last = path[path.Count - 1];
            switch (op)
            {
                case "set": cur[last] = MostJson.Clone(value); break;
                case "merge":
                {
                    if (!cur.TryGetValue(last, out var e)) e = new Dictionary<string, object>();
                    Merge(ref e, value);
                    cur[last] = e;
                    break;
                }
                case "del": cur.Remove(last); break;
            }
        }

        static void Merge(ref object a, object b)
        {
            if (a is Dictionary<string, object> ad && b is Dictionary<string, object> bd)
            {
                foreach (var kv in bd)
                {
                    if (kv.Value == null) { ad.Remove(kv.Key); continue; }
                    ad.TryGetValue(kv.Key, out var cur);
                    Merge(ref cur, kv.Value);
                    ad[kv.Key] = cur;
                }
                return;
            }
            a = MostJson.Clone(b);
        }
    }
}
