# Мост — SDK приложений для Unity

## Установка

**Window → Package Manager → «+» → Add package from git URL…** и вставьте:

```
https://github.com/exerussus/bridge.git?path=/sdk/unity/com.exerussus.most
```

Закрепить версию — тегом или веткой в конце (если тег есть в репозитории): `…?path=/sdk/unity/com.exerussus.most#v3.5.0`.
Или вручную в `Packages/manifest.json`:

```json
"com.exerussus.most": "https://github.com/exerussus/bridge.git?path=/sdk/unity/com.exerussus.most"
```

Нужен Unity 2022.3 или новее и мост 3.5 или новее.

---

Пакет делает вашу игру приложением моста: сеанс — с теми, кто сейчас в вашем голосовом канале. Без серверов, без
лобби и без привязки к сетевой библиотеке. Вы шлёте байты, а мост доставляет их надёжно, быстро или «последнее по ключу».
Формат сообщений — ваш.

## За минуту

```csharp
using Exerussus.Most;

async void Start()
{
    var most = Most.Connect();
    await most.WhenOpen;                       // мост ответил
    await most.Session.JoinOrCreate();         // в идущий сеанс в канале — или свой

    most.Session.OnData += d => Debug.Log($"#{d.From}: {d.Text()}");
    most.Session.SendText(MostTo.All, "привет");
}

void Update()
{
    var s = Most.Session;
    if (s != null && s.InSession)
        s.Send(MostTo.Others, MyPositionBytes(), MostDelivery.Latest, key: 1);
}
```

## Как это устроено

- **Канал — это лобби.** Хозяин ПК сидит в голосовом канале. Сеансы видны и доступны только тем, кто в том же канале.
  `JoinOrCreate` входит в идущий сеанс этого приложения или начинает свой. Если вышли из канала, вы выходите и из сеанса
  (это отключается в app.json: `"net": {"leaveWithChannel": false}`).
- **Звезда через хоста.** Хост — ПК, начавший сеанс, у него `cid = 0`. Остальные ходят к нему через свои мосты, по уже
  подписанной связи мостов. Адресат — `MostTo.Host`, `MostTo.All`, `MostTo.Others` или номер участника.
- **Доставка:**

  | Способ | Предел | Когда |
  |---|---|---|
  | `Reliable` | 1 МБ | события, команды, чат, снимки |
  | `Fast` | 1200 Б | частые мелочи; при перегрузке теряется |
  | `Latest` + key 0–255 | 1200 Б | позиции, курсоры: устаревшее не доставляется |

  Сейчас «быстро» и «последнее» тоже идут по связи мостов (WebSocket), но отбрасываются и сливаются, когда канал занят.
  Отдельный UDP — в планах, API от этого не изменится.
- **Общее состояние** (`Session.State`) — JSON-дерево у хоста (до 4 МБ). Операции `Set`, `Merge` и `Delete` по пути
  `"players.7.hp"`; хост применяет их по порядку, у всех одинаковый `Rev`. Вошедший позже получает всё дерево сразу.
- **Присутствие** (`Presence`, до 4 КБ) — «что я сейчас делаю». Рассылается всем и не хранится.
- **Часы сеанса** (`Session.Time`) — часы хоста в мс, общие для всех. `Session.Rtt()` — задержка до хоста.

## Откуда берётся доступ

| Где запущено | Как |
|---|---|
| Своё окно, запущенное мостом | мост передаёт `MOST_API` и `MOST_TOKEN` в окружении |
| Редактор Unity | **Мост → Подключение…** → «Подключить редактор»; хозяин ПК подтверждает в панели моста. Токен хранится в EditorPrefs (не в проекте) до перезапуска моста |
| WebGL в мосте | игра в изолированной рамке, вызовы идут через панель (`window.most`); сборка — «Мост → Собрать приложение (WebGL в мосте)» |

Приложение должно лежать в папке «приложения» моста и быть разрешено хозяином ПК, иначе подключения нет.
Пока ПК управляют по сети, сеансы не начинаются. Приложения не могут управлять ПК и менять настройки моста.

## Первый запуск из редактора

1. **Мост → Подключение…** → «Папка»: создайте папку внутри «приложения» моста (например, `приложения/eggs`).
2. Нажмите «Создать app.json (сетевое приложение)». Мост покажет приложение и попросит разрешить его — разрешите.
3. «Подключить редактор» → подтвердите в панели моста.
4. Нажмите Play. Второй игрок — сборка на другом ПК в том же канале или второй редактор на соседнем ПК.

## Сборка

«Мост → Собрать приложение (своё окно)» собирает игру в `<папка>/bin/` и прописывает `native.cmd` в app.json.
WebGL-сборка ложится в `<папка>/ui/`, и в index.html подключается `/most/sdk.js`. На время сборки выключаются сжатие
и кэш данных: рамка изолирована, а мост не отдаёт `Content-Encoding`.

После новой сборки код меняется, и мост попросит разрешить приложение заново. Так задумано.

## app.json

```json
{
  "id": "eggs",
  "name": "Яйца",
  "version": "0.1.0",
  "native": { "cmd": "bin/Eggs.exe" },
  "net": { "proto": 1, "max": 8 },
  "permissions": ["sessions", "activity", "notify"],
  "bridge": "apps@1"
}
```

`net.proto` — версия вашего протокола. Войти можно только в сеанс с тем же `proto`; меняйте его, когда старые сборки
не поймут новые.

Права:
- `sessions` — сеансы;
- `activity` — журнал активности;
- `notify` — уведомления.

## Справка по API

```csharp
// подключение
MostClient c = Most.Connect(new MostOptions { Name = "окно 2" });
await c.WhenOpen;            // MostException с причиной, если мост не пустил
c.Channel                    // Room, Title, Members, InChannel
c.OnChannel / OnClosed / OnError
c.Simulation = new MostSimulation { Enabled = true, LatencyMs = 120, JitterMs = 30, Loss = 0.05f };

// сеанс
await c.Session.JoinOrCreate(new MostSessionOptions { Max = 4, Meta = ..., State = ... });
await c.Session.Create(); await c.Session.Join(sid); List<MostSessionInfo> l = await c.Session.List();
await c.Session.Leave();
c.Session.Id, Me, IsHost, Host, Members, Max, Room, Title
c.Session.Send(to, bytes, MostDelivery.Reliable | Fast | Latest, key);
c.Session.SendText / SendJson
c.Session.OnJoined, OnMember, OnLeft, OnData, OnPresence, OnEnded, OnSessions, OnRtt, OnConnect
await c.Session.Presence(obj);
c.Session.Time; c.Session.Rtt(); c.Session.Rtt(cid);
await c.Session.Kick(cid);                     // хост

// общее состояние
await c.Session.State.Set("score", 10);
await c.Session.State.Merge("players.3", new Dictionary<string, object> { ["hp"] = 7L, ["buff"] = null }); // null — удалить поле
await c.Session.State.Delete("players.3");
c.Session.State.GetLong("score"); c.Session.State.Get<MyData>("players.3"); c.Session.State.OnChanged += (ops, by) => ...

// свой сервер (Transport = "external")
await c.Session.SetConnect("192.168.1.5:7777"); // хост; придёт всем в OnConnect
c.Session.Ticket                                 // участник передаёт своему серверу
await c.CheckTicket(ticket);                     // сервер на ПК хоста проверяет: {ok, who: {sid, cid, node, name}}

// вне сеанса
await c.StoreSet("save", obj); var v = await c.StoreGet<MySave>("save");   // до 1 МБ на приложение
await c.Notify("Яйцо вылупилось!"); await c.Toast("Сохранено");

// журнал активности (право activity): минуты в канале и разговора, сообщения, показ экрана, пинги, отправки
var r = await c.Activity.Since(cursor);  // {events: [...], cursor, more} — храните cursor в StoreSet
var t = await c.Activity.Stats("today"); // {voiceMinutes, talkMinutes, counts}
c.Activity.OnEvent += e => ...;
```

Все события приходят в главном потоке. Их разбирает невидимый объект «[Мост]», который создаётся сам при `Connect` в
режиме игры. Вне режима игры зовите `Most.Pump()` сами.

## Своя сетевая библиотека

SDK ничего не навязывает. Чтобы работать через FishNet, Mirror или NGO, напишите тонкий транспорт поверх
`Session.Send` и `Session.OnData`:
- «сервер» — это хост (`IsHost`), «клиенты» — участники;
- `Reliable` и `Fast` соответствуют надёжному и ненадёжному каналу;
- подключения — это `OnMember` и `OnLeft`.

Если хватает одного широковещательного канала со своими сообщениями, смотрите пример «Свои сообщения»: первый байт —
тип сообщения, дальше тело в вашем формате.

## Примеры

Package Manager → «Мост — SDK приложений» → Samples:
- **Курсоры** — курсоры всех в канале («последнее» по ключу), точки по щелчку и общий счёт;
- **Свои сообщения** — типы сообщений поверх одного канала.

## Имитация сети

**Мост → Подключение…** → «Имитация сети»: задержка, разброс и потери «быстро» для входящего в режиме игры.
Надёжное не теряется и не меняет порядок.
