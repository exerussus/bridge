# API моста для ассистента

Всё, что умеет панель, доступно программе на этом же ПК: локальный HTTP, поток событий и инструменты для Claude (MCP).

## Подключение

- Адрес: `http://127.0.0.1:<порт>/bridge/v1/local/<вызов>` (порт — из `listen` в bridge.json, обычно 5390).
- Ключ: заголовок `X-Bridge-Local: <содержимое bridge-data/local.token>`. Для потоков (EventSource) можно `?token=…`.
- Только с этого ПК (127.0.0.1). Ответы — JSON; ошибка — HTTP-код и `{"error": "понятный текст"}`.
- **Пока соседу разрешено управлять этим ПК, мост заперт:** любые изменения (не GET) отвечают `423` с понятным текстом. Работают только голос, показ, доска, чаты, `control/*` (в том числе `control/revoke`) и чтение.
- Перечень вызовов от самого узла: `GET api`. Из консоли: `bridge api GET|POST <вызов> [json | @файл]`.

```python
import requests
B = "http://127.0.0.1:5390/bridge/v1/local/"
H = {"X-Bridge-Local": open(r"C:\Bridge\bridge-data\local.token", encoding="utf-8-sig").read().strip()}
requests.post(B + "ui", json={"action": "join", "channel": "Семья"}, headers=H).json()
```

## События (что происходит — без опроса)

`GET events` — поток Server-Sent Events, каждое событие: `event: bridge`, `data: {"kind", "time", "data"}`.

| kind | Когда | data |
|---|---|---|
| `peer-online` / `peer-offline` | сосед появился / пропал | `peer`, `error` |
| `peer-named` | новый сосед назвался | `name`, `temp` (временное имя) |
| `file-in` | пришёл файл: в папку соседа или в пространство | `mount` или `share`+`peer`, `path` |
| `mailbox-received` / `mailbox-sent` | ручная отправка: получено / доставлено | `peer`, `path`, `size` |
| `folder-write` | сосед записал в вашу общую папку | `peer`, `folder`, `path` |
| `record` / `records-in` | изменилась запись здесь / пришли записи от соседа | `share`, `record` / `share`, `peer`, `count` |
| `command-waiting` | команда соседа ждёт подтверждения | кто, что, id записи |
| `media-invite` | звонок или просьба показать экран | `from`, `title`, `media`, `room` |
| `room-join` / `room-leave` / `room-state` | кто-то зашёл в канал / вышел / поменял состояние (микрофон, звук, показ) | `room`, `peer`, `state` |
| `agent` | поручение агенту: пришло, готово, не получилось | `thread`, `role`, `peer`, `status`, `title` |
| `device-paired` | подключён телефон | `id`, `name` |
| `chat-msg` | пришло сообщение в чат | `conv`, `from`, `author`, `text`, `dm` |
| `assistant-in` | (хозяин) пришло сообщение ассистенту | `peer`, `kind`, `id`, `seq` |
| `assistant-msg` | ответ или уведомление ассистента пришло на этот ПК | `id`, `kind`, `notice`, `alarm`, `text` (начало), `unread` |
| `assistant-state` | ассистент появился / пропал | `online` |
| `device-outbox` | файл положен «на телефон» | `device`, `path` |
| `update-offer` | сосед предлагает обновить мост | `from`, `version` |
| `update-state` | ход обновления | `status` (offer, download, install, restart, done, error), `error` |
| `update-done` | мост запустился после обновления | `version`, `from` |
| `peer-version` | сосед перешёл на другую версию | `peer`, `version`, `was` |

## Голос и экран — пульт панели

Голос и экран работают в окне панели (это браузер). Ассистент управляет ими командой `POST ui {action, …}`: мост передаёт её панели и возвращает результат. Если панель закрыта, мост откроет её сам. Если открыто несколько окон, команду выполнит одно: то, что в канале, иначе окно в фокусе.

| action | Параметры | Что делает |
|---|---|---|
| `state` | — | где вы сейчас: канал, участники и их состояние, ваши показы, что смотрите |
| `join` | `channel` (название или id), `quiet?` | зайти в канал; `quiet: true` — тихо, без звука входа у остальных |
| `leave` | — | выйти |
| `mic` | `on` | микрофон вкл/выкл |
| `deafen` | `on` | не слышать никого (и не говорить) |
| `volume` | `peer`, `value` 0…1 | громкость участника |
| `call` | `peers: [...]`, `title?` | позвонить без канала — у них зазвонит |
| `answer` | `room?` | ответить на входящий звонок (последний, если не указан) |
| `decline` | `room?` | отклонить |
| `share` | `quality?`: `text` / `motion` / `low` | показать экран. Окно выбора «весь экран / окно / вкладка» браузер показывает всегда — нужен один щелчок человека; ждём до 120 с |
| `unshare` | `id?` | остановить показ (все, если не указан) |
| `watch` | `peer` или `stream`, `on?` | смотреть показ участника (в панели) |
| `popout` | `peer` или `stream` | открыть показ в отдельном окне |
| `ask_screen` | `peer` | попросить соседа показать экран |
| `tab` | `tab` | открыть вкладку панели (`home`, `peers`, `send`, `folders`, `shares`, `records`, `voice`, `agent`, `assistant`, `phone`, `access`, `log`, `help`) |

Голос с телефона идёт тем же каналом: телефон — то же «я», что этот ПК (зашли с телефона — панель уступает). Ретранслятор (TURN, UDP `turnPort`, по умолчанию 3478) соединяет участников, которые не видят друг друга напрямую: `GET media/ice` — адреса и временные ключи ретрансляторов для WebRTC.

Каналы (без панели): `GET channels` (свои и видимые у соседей, кто где), `POST channels {id, title, members: [сосед | @группа | *]}`, `POST channels/remove?id`, `GET media/presence`, `GET media/invites` (входящие звонки).

## Файлы

- **Отправить соседу файл с диска**: `POST mailbox/send {peer, path, dest?}`. Байтами: `POST mailbox/upload?peer&path` (тело — файл).
- Очередь отправки: `GET mailbox/outgoing`, отменить: `POST mailbox/cancel?peer&path`. Пришедшее: `GET mailbox/incoming`, скачать: `GET mailbox/incoming/file?path`.
- **Общие папки**: `GET folders` (свои и соседей), `POST folders {id, title, path, access: {сосед | @группа | *: read | write}}`, `POST folders/remove?id`.
- Файлы в общей папке: `GET fs/list?kind=own|mount&id[&peer]`, `GET fs/file?…&path`, `POST fs/upload?…&path` (тело — файл), `POST fs/delete?…&path`. Корзина: `GET trash`, `POST trash/restore?scope&batch&path`.
- Игнор: `GET ignore`, `POST ignore/add|remove {scope, id, peer, pattern}`, `GET ignore/test?path`.
- Ход передач: `GET transfers` (кто, что, сколько из скольки, скорость, осталось).
- Настройки передачи: `POST folders/settings?upMbit&downMbit&maxFileMb&trashMinutes`.

## Соседи, группы, права

- `GET peers` (связь, версия, агент), `POST peers/invite?title` → строка «адрес ключ», `POST peers/connect {address, secret?}`, `GET peers/check[?name]` (почему нет связи — по шагам), `POST peers/rename?name&to`, `POST peers/remove?name`, `POST peers {name, enabled?, shares?, …}`.
- Группы: `GET groups`, `POST groups {id, title, members}`, `POST groups/remove?id`. Кто что видит: `GET access`.

## Задачи и записи

- `GET records[?share&type&status&assignee&q&deleted=1]`, `GET records/{id}`, `GET records/{id}/history`.
- `POST actions {action, share?, record?, …}` — `create`, `set`, `transition`, `comment`, `ack`, `attach`, `link`, `approve`, `reject`, `delete`, `restore`.
- Долгое ожидание новых событий: `GET wait?share&after&timeout`.

## Телефоны

- `GET devices` — подключённые телефоны (имя, доступ, на связи ли).
- `POST devices/pair {rights?: "full" | "files"}` → `{code, expiresIn, urls: [{label, url, qr}]}` — код и QR (SVG) для подключения; код одноразовый, 10 минут.
- `POST devices {id, name?, rights?}`, `POST devices/remove?id` — переименовать, сменить доступ, отключить.
- **Отправить файл на телефон**: `POST devices/send {device?, path, name?}` (файл с диска; `device` можно не указывать, если телефон один). Байтами: `POST devices/upload?device&path`.
- `GET mobile` — телефоны, адреса для подключения, состояние Tailscale; `POST mobile/https {on}` — HTTPS через Tailscale.
- Файлы с телефона приходят как от соседа: событие `mailbox-received`, `peer` — имя телефона, файл — в «приём/<имя телефона>».

## Ассистент (контракт v1)

Ассистент — программа на одном ПК семьи («хозяин»). Она забирает входящие у своего моста и отдаёт ответы; мосты других ПК показывают вкладку «Ассистент» и пересылают сообщения хозяину по протоколу соседей.

| Вызов | Что |
|---|---|
| `GET assistant/info` | `{version: 1, seq}` — контракт поддерживается; `seq` — номер последнего входящего |
| `GET assistant/wait?after=<seq>&timeout=<≤60>` | долгий опрос: `{seq, items: [...]}`; `after` — подтверждение (всё ≤ after мост больше не хранит) |
| `POST assistant/reply {id, to, kind, replyTo?, text, files?, buttons?}` | ответ на ПК `to` (`reply` · `notice` · `progress`): `{ok, delivered}`; ПК не на связи — `{ok, delivered: false, queued: true}` (notice хранится 24 ч, reply — 7 дней, progress не копится); неизвестный ПК — 404 |
| `POST assistant/presence {online, title, version}` | ассистент жив (раз в минуту); `online: false` — ушёл |

Входящее: `{seq, id, time, peer, kind: text|voice|file|action, text, files?: [{name, mime, size, data}], voice?: {mime: "audio/wav", data}, action?: {name, args}, replyTo}`. `peer` — имя ПК, на котором написали; его ставит мост хозяина по подписи соседа. Голос из панели — WAV PCM16, моно, 16 кГц, до 120 с. Файлы — до 10 МБ, base64.

Очередь хозяина — `bridge-data/assistant/inbound.jsonl` (7 дней, до 1000 сообщений), переживает перезапуск; повтор с тем же `id` — не дубль. Кто может писать ассистенту: `assistant.allowFrom` в bridge.json (по умолчанию `["*"]` — все включённые соседи).

Для панели (на любом ПК): `GET assistant/chat[?after&timeout]` — история, «печатает…», на связи ли ассистент; `POST assistant/send {kind, text, files?, voice?, action?, replyTo?}`; `POST assistant/read`. Файлы из ответов — в «приём/ассистент» (`GET mailbox/incoming/file?path=ассистент/<имя>`). Пульт: `POST ui {action: "tab", tab: "assistant"}`.

Исполнители соседа, которые этому ПК можно запускать, — `peers[].runners` в `GET overview`: `[{name, title, share, params, hint, confirm}]`. `hint` — новое поле исполнителя в bridge.json: подсказка людям и ассистенту, какие значения допустимы.

## Агенты (Claude поручает Claude)

(3.3.1) Вопросы людям: `POST agent/request {thread, question, buttons?, input?}` → `{id, …}` (нить исполнителя в работе; статус — «ожидание»), `GET agent/request/wait?thread&id&timeout` → `{answered, req}`, `POST agent/answer {thread, req, answer}` (у заказчика уходит исполнителю; кнопочный вопрос принимает только кнопку; второй ответ — 409). В нити — `requests: [{id, q, buttons, input, at, answer, by, answered}]`, `open` — сколько без ответа. MCP: `agent_request {question, buttons?, input?, thread?, wait_seconds?}`; Claude, которого запускает мост, получает id нити в `BRIDGE_AGENT_THREAD` и инструменты моста (`--mcp-config`).

`GET agent` (настройки, нити, у кого агент), `POST agent/ask {peer, prompt, thread?, title?, record?}` (`peer` пусто или `local` — Claude на этом ПК, нить `local: true`; в `GET agent` — `local {name, mode}`), `GET agent/wait?id&after&timeout`, `GET agent/thread?id&after`, `POST agent/stop?id`, `POST agent/settings {…}`, `POST agent/exchange?peer`. Ручной режим исполнителя: `GET agent/wait-task`, `POST agent/take?id`, `POST agent/progress {thread, text?, phase?, waiting?}`, `POST agent/report {thread, result, ok}`.

- Статус нити: `queued` — ждёт, пока возьмут; `running` — в работе; `waiting` — в работе, но явно ждёт (сборка, ответ); `idle` — ход завершён, можно продолжить; `failed`; `stopped`. Пока исполнитель работает над новым поручением, ход прошлого (пришедший позже) статус и итог не меняет (`asks` — сколько поручений в нити).
- `phase` и `phaseAt` — чем агент занят сейчас, он пишет это сам на смену этапа: в авто-режиме строкой «Этап: …» или «Ожидание: …» в своём сообщении (мост просит об этом в инструкции), в ручном — `agent/progress {phase, waiting}`. В ходе — событие `kind: "phase"`.
- `record: {share, id}` — поручение решает запись-задачу. Мост исполнителя сам ведёт её статус: в работе / ожидание → «принята» → «в работе», ход завершён → «на проверке» (итог — в пояснении), не получилось или остановлено — комментарий. «Готово» ставит только заказчик. Для этого исполнитель задачи (`assignee`) — ПК агента, и пространство есть у него. Панель пишет в задачу поля `agent` (ПК агента) и `agentThread` (нить).

## Чаты

Беседа: `dm:<сосед>` (личная) или `ch:<владелец>:<канал>` (канала). Сообщения хранятся сутки; кто не на связи — получит, пока сообщению меньше суток.

- `GET chat` — беседы: `{conversations: [{conv, kind: channel|dm, title, peer|owner, online, last, unread}], unread}`.
- `GET chat/messages?conv&after&timeout` — сообщения новее номера `after` (без `conv` — всех бесед), долгий опрос до 30 с: `{items: [{n, id, conv, from, author, time, text, mine, status}], n}`.
- `POST chat/send {to: сосед | channel: id или название | conv, text, author?, files?, sticker?, reply?}` — написать от имени этого ПК; `reply: {id, from, author?, text, q?}` — ответ на сообщение (`text` — начало или выделенная цитата до 300 знаков, `q: true` — это цитата; у сообщения поле `reply` приходит всем); `author` — подпись («Ассистент»); `files` — `[{name, mime, data(base64)}]` или `[{path}]` (файл с диска), до 10 МБ на файл. Ответ `{ok, id, delivered}`; `delivered: false` — в очереди.
- `sticker` — id своего (или присланного) стикера: уйдёт вложением с пометкой `sticker: true`.
- `GET chat/file?id&name` — вложение сообщения (картинка — для показа, `&dl=1` — скачать).
- `POST chat/read?conv` — прочитано.
- Событие `chat-msg` в `GET events`: `{conv, from, author, id, text, dm}` — пришло сообщение (для ретрансляции, например, в Telegram).

## Стикеры

У каждого свой набор (картинки и гифки до 2 МБ, хранятся на этом ПК). Присланный стикер получатель сохраняет отдельно — в «Присланные» (последние 300), он живёт дольше суток чата.

- `GET stickers` — `{mine: [{id, size}], got: [{id, size, from, time}], max}`; id — отпечаток и расширение, например `3fa9c2d1e0b7.gif`.
- `POST stickers` (тело — картинка или гифка) → `{id}`; `POST stickers/remove {id, set: mine|got}`; `POST stickers/keep {id}` — присланный в свои.
- `GET sticker?id[&set]` — картинка.

## Звук входа в канал

`POST joinsound` (тело — звук MP3/OGG/WAV/WebM/M4A до 1 МБ), `POST joinsound/remove`, `GET joinsound[?peer]` — свой или соседа. Отпечатки — в `GET avatars`: `{sound, sounds: {сосед: отпечаток}}`. Панели участников канала проигрывают звук входящего (до 6 с); у каждого в «Настройках» можно выключить чужие звуки и задать их громкость.

## Пространства

- `POST shares {id, title, folder, peers, accept: [{pattern, from}], recordsAccept: [{from, types, ops}]}` — создать или изменить пространство (папка по умолчанию — «Пространства/<id>»; сменили папку — пространство откроется заново, файлы не переносятся). `peers` — соседи или `@группа`. `accept` — что принимать: первое подходящее правило; пусто — только раздавать. `recordsAccept` пусто — записи от всех участников.
- `POST shares/remove?id` — убрать пространство с этого ПК (папка остаётся).
- `GET shares/offers` — `{offers: [{peer, id}]}`: пространства, которые соседи открыли этому ПК, а здесь их нет.
- Записи: `POST actions {action: "create", share, type, fields}` — любой вид (`task`, `decision`, `note`, `status` или свой); `{action: "set", record, fields, expectVersion}` — изменить поля (null — убрать поле).

## Доска канала

Доска есть только у каналов (не у звонков без канала) и живёт у владельца канала. `room` — «владелец:канал».
Объект — JSON с `id` (буквы, цифры, `-`, `_`), `t` (path, rect, arrow, text, note, img) и полями панели; картинка — `{t: "img", src: id, x, y, w, h}`. У текста и заметки (3.1): `fs` — размер, `ff` — шрифт (`simple`, `tech`, `hand`; нет — Мост), `fb`, `fi`, `fu` — жирный, курсив, подчёркнутый, `fa` — `center`/`right`.

- `GET board/state?room` — снимок: `{room, owner, enabled, edit: all|owner, seq, objs, bg, layers}`. Стенды сохраняют фон и слои.
- `GET board/poll?room&after&timeout` — изменения после номера `after` (долгий опрос до 30 с): `{seq, events: [{seq, by, cs, op}]}`; `resync: true` — перечитайте снимок.
- `POST board/op {room, cs, ops}` — изменения: `{t: "put", o}` (у объекта `L` — id слоя, по умолчанию `base`), `{t: "del", ids}`, `{t: "clear"}`, `{t: "load", objs}`, `{t: "enable", on}`, `{t: "edit", mode}` (только владелец), `{t: "bg", c, l}` (фон: `c` — `#rrggbb` или пусто, `l` — яркость 20–200 %), `{t: "layers", list: [{id, name}]}` (слои по порядку снизу вверх, 1–64; скрытие слоя — только у себя), `{t: "cur", x, y, vx, vy, vz}` (курсор и вид — не хранится). `cs` — номер сеанса: свои изменения панель узнаёт по нему.
- `POST board/image?room` (тело — картинка до 12 МБ) → `{id}`; `GET board/image?room&id`.
- Стенды (на этом ПК): `GET board/stands`, `POST board/stand/save {room, name}`, `POST board/stand/load {room, id, cs}` (доска сменится у всех), `GET board/stand/file?id` (файл `.mostboard`), `POST board/stand/import?name` (тело — файл), `POST board/stand/remove {id}`.
- Соседи: `GET /peer/board/state?room`, `POST /peer/board/op`, `POST /peer/board/sub`, `POST /peer/board/push` (владелец → участникам), `GET|POST /peer/board/image`.

### Доска для ассистента — понятные вызовы

Работают и у владельца канала, и у участника (тогда через владельца). `room` — «владелец:канал», id или название канала. Координаты — единицы доски (как «x · y» внизу доски), y — вниз. Пока доска выключена, изменения отвечают `409` — включите `board/enable`.

- `GET board/rooms` — доски (каналы), видимые с этого ПК.
- `GET board/describe?room` — обзор: включена ли, кто правит, фон, слои (сколько объектов в каждом), границы занятого, сколько чего, шрифты.
- `GET board/find?room&q&type&layer&by&x0&y0&x1&y1&limit` — поиск: `q` — по тексту (и имени PDF), `type` — `text,note,shape,link,rect,arrow,path,image` (через запятую; у фигуры — `kind`, у связи — `from`, `to`, `kind`, `start`, `end`), `layer` — id или название, `by` — автор, `x0..y1` — область. Ответ — `{total, objs: [{id, type, layer, layerName, box: [x0, y0, x1, y1], text?, color?, size?, font?, bold?, …}]}` сверху вниз, слева направо. Рамка текста — оценка (точно знает только панель).
- `GET board/get?room&ids=a,b` — объекты целиком (как хранятся) и рамки; `missing` — каких нет.
- `POST board/add {room, objs: [...], layer?, at?: [x, y], layout?: row|column|grid, gap?, rowWidth?}` — положить. Объект — `{type, id?, layer?, x?, y?, …}`. Без `x, y` объекты встают рядом с тем, что уже есть (или от `at`): в ряд, столбиком или сеткой.
  - `text`: `text`, `size` (24), `color`, `font` (`default` — Мост, `simple` — Golos Text, `tech` — Tektur, `hand` — Caveat), `bold`, `italic`, `underline`, `align` (`left|center|right`).
  - `note`: то же, `w` (240), `color` — цвет листка, `textColor`.
  - `rect`: `w`, `h`, `color` (контур), `fill`, `width` (толщина).
  - `arrow`: `x, y, x2, y2` или `from: [x, y], to: [x, y]`, `color`, `width`. `line`: `x, y, x2, y2`. `path`: `points: [[x, y], …]`, `color`, `width`, `opacity`.
  - `image`: `path` (файл на этом ПК) или `data` (base64), `w` и/или `h` (пропорции сохраняются), иначе по картинке, но не больше `maxSize` (800). По ссылке мост не скачивает — сохраните файл и передайте `path`.
  - `pdf`: `path` или `data`, `pages` (`"1-3,5"`), `w` (800), `maxPages` (40). Страницы рисует панель (если закрыта — мост откроет), каждая — картинка на доске.
  - `shape` (3.3) — фигура диаграммы: `kind` (`rect`, `round`, `ellipse`, `diamond`, `triangle`, `hexagon`, `parallelogram`, `trapezoid`; блок-схема — `terminal`, `document`, `database`, `predefined`, `manual`, `delay`, `offpage`; ещё — `cloud`, `callout`, `star`, `arrowR`, `cross`; контейнеры — `frame`, `lane`), `w`, `h`, `text` (внутри, по центру; `valign` — `top|middle|bottom`), шрифт как у `text`, `fill`, `color` (контур), `textColor`, `width`, `dash` (`solid|dash|dot`), `opacity`. `frame` и `lane` можно и типом: их перетаскивают вместе со всем, что внутри.
  - `link` (3.3) — связь: `from`, `to` — id объекта (можно из этого же `add`) или точка `[x, y]`; `fromSide`, `toSide` — `top|right|bottom|left|auto`; `kind` — `straight` (по умолчанию), `elbow` (ломаная), `curve`; `start`, `end` — `none|arrow|triangle|diamond|circle|one|many` (по умолчанию `none` и `arrow`); `label`, `labelAt` (0…1, где подпись); `color`, `width`, `dash`. Связь едет за фигурами; удалили фигуру — уходят и её связи.
  - `diagram` (3.3): `mermaid` — текст flowchart (`flowchart TD`, узлы `A[…]`, `B{…}`, `C([…])`, `D[(…)]`, `E((…))`, связи `-->`, `-.->`, `==>`, `-->|подпись|`, `subgraph … end`). Панель разбирает и раскладывает (если закрыта — мост откроет); в ответе — фигуры и связи.
  - Цвет — `#rrggbb`, `#rgb` или имя: white, black, cyan, red, orange, yellow, green, purple, blue, gray, pink (и по-русски). По умолчанию текст светлый, на светлом фоне — тёмный.
  - Ответ — `{seq, objs: [кратко, с рамками]}`; занятый `id` — `409`.
- `POST board/edit {room, id | ids, …те же поля, layer?, toFront?}` — поменять текст, цвет, размер, шрифт, стиль, рамку, слой; у картинки — `path`/`data` (заменить картинку); у связи — `from`, `to`, стороны, вид, концы, подпись.
- `POST board/move {room, ids, dx, dy}` — сдвинуть; `{room, ids, x, y}` — поставить левый верхний угол группы в точку.
- `POST board/remove {room, ids}`; `POST board/clear {room, layer?}` — всё или только слой.
- `POST board/layer {room, action, layer?, name?, to?}` — `add {name}`, `rename {layer, to}`, `remove {layer}` (вместе с рисунками), `up`, `down`, `top`, `bottom`. Слои — снизу вверх; то, что кладёт ассистент без `layer`, ложится в верхний.
- `POST board/bg {room, color?, light?}` — фон (`""` — по умолчанию) и яркость 20–200 %. `POST board/enable {room, on}`.
- Пульт панели (панель должна быть в этом канале — `ui join`): `POST ui {action: "board_png", room, ids? | area?: [x0, y0, x1, y1], what?: "vis", scale?, bg?: "light"|"none"}` → `{path, width, height, area}` — PNG в `bridge-data/shots/` (последние 50); `POST ui {action: "board_view", room, ids? | area? | x, y, zoom?, full?}` — показать место человеку (выделит `ids`).

## Свой звук пинга

`POST pingsound` (тело — звук до 300 КБ), `POST pingsound/remove`, `GET pingsound[?peer]` (соседа — его слышит тот, кто его пингует; отпечатки — `pings` в `GET avatars`). Его слышите только вы, когда вас пингуют; отпечаток — в `GET avatars` (`ping`).

## Показ экрана — для ассистента

Через пульт панели (панель — в канале, где идёт показ; список показов — `ui state`):

- `POST ui {action: "screen_shot", peer | stream | mine: true, maxSize?: 1920, marks?: true, format?: "jpeg"}` → `{path, width, height, peer, stream}` — кадр показа в файл (`bridge-data/shots/`). Если показ ещё не смотрели — панель включит просмотр на время снимка (`keep: true` — оставить).
- `POST ui {action: "screen_point", peer, x, y}` — пинг точки (x, y — доли экрана 0…1, 0,0 — левый верх).
- `POST ui {action: "screen_draw", peer, points: [[x, y], …]}` — линия поверх (гаснет через несколько секунд), `screen_clear {peer}` — стереть свои пометки.
- Пометки видят все, кто смотрит, и показывающий (у него — прямо на экране). Управлять чужим ПК ассистент не может: только человек — запросом и подтверждением у показывающего.

## Заглушили и управление ПК

- `POST self/muted {by}` — панель сообщает, что этот ПК заглушили (`by` — кто; пусто — снова слышно). Трей показывает это значком.
- `GET control/state` — `{muted, control: {peer, keyboard, seconds} | null, supported}`.
- `POST control/grant {peer, keyboard, w, h}` — разрешить соседу управление (панель продлевает раз в 8 с; без продления разрешение гаснет через 30 с). `w`, `h` — размер показываемого монитора.
- `POST control/input {peer, events}` — события `{t: m|d|u|w|kd|ku, x, y, b, dx, dy, c}` (x, y — доли монитора, c — KeyboardEvent.code); выполняются, только если разрешение есть и сосед тот же; клавиши — только если разрешена клавиатура.
- `POST control/revoke` — отозвать (нажатые клавиши и кнопки отпускаются).
- `POST overlay/ann {by, col, a, w, h}` — пометка зрителя поверх экрана (рисует трей).
- Ничего из этого недоступно с телефона.

## Обновление моста

«Поделиться версией» (окно «Что нового»): соседям со старой версией уходит предложение; обновляются они только после подтверждения у себя.

- `POST update/offer {peers?}` — предложить (без списка — всем на связи) → `{version, results: {сосед: что вышло}}`. Мостам до 2.12 файлы уходят обычной отправкой (в «приём/<вы>/Мост <версия>/») и сообщением в личный чат.
- `GET update/state` — `{version, status, error, offer: {from, version, files, notes}, restartable}`.
- `POST update/accept` — скачать у предложившего Мост.exe и bridge.exe, сверить sha256, заменить (старые — в `*.old`, их уберёт следующий запуск) и перезапуститься; `POST update/decline`.
- Соседи: `POST /peer/update/offer {version, files: [{name, size, sha256}], notes}`, `GET /peer/update/file?name` (только файлы программы).

## Плитка участника

Как вас видят в канале (`Настройки` → `Плитка и аватарка`). Хранится в мосте (`bridge-data/tile.json`, `tile.img`), соседи забирают по отпечатку (`tile` в привете), как аватарку.

- `GET tile[?peer]` → `{style, img}` (`img` — отпечаток картинки-фона, пусто — нет). `GET tile/image[?peer]` — картинка.
- `POST tile {style}` — стиль: `fit` (`cover|contain|repeat`), `x`, `y` (0–100 — точка фокуса), `zoom` (100–400), `mask` (`none|bottom|top|sides|vignette|circle|diagonal`), `soft` (0–100), `opacity` (5–100), `blur` (0–20), `tint` + `tintAmt` (0–100), `base` (цвет плитки), `border`, `glow` (подсветка, когда говорите), `name` (цвет имени), `avShape` (`circle|round|square|hex|none`), `avSize` (50–160), `avRing`. Цвета — `#rrggbb`; неизвестное отбрасывается.
- `POST tile/image` (тело — JPEG/PNG/WebP/GIF до 4 МБ; гифка остаётся анимированной), `POST tile/image/remove`, `POST tile/reset`.
- `GET avatars` — ещё и `tile` (свой отпечаток) и `tiles` (соседей). Соседи: `GET /peer/tile`, `GET /peer/tile/img`.

## Оформление канала

Иконка и подложка канала (3.2) — у владельца: `bridge-data/chan-art/<канал>/`. Участники получают отпечаток `art` в описании канала и забирают оформление один раз.

- `GET chan/art?room` → `{icon, back, style}` (`icon`, `back` — отпечатки картинок; `room` — «владелец:канал»).
- `GET chan/img?room&what=icon|back` — картинка.
- Свой канал: `POST chan/img?id&what=icon|back` (тело — JPEG/PNG/WebP/GIF; иконка до 2 МБ, подложка до 4 МБ), `POST chan/img/remove {id, what}`, `POST chan/style {id, style}` — подложка: те же ключи, что у плитки (`fit`, `x`, `y`, `zoom`, `mask`, `soft`, `opacity`, `blur`, `tint`, `tintAmt`).
- Соседи: `GET /peer/chan/art?id`, `GET /peer/chan/img?id&what` — только участникам канала.

## Аватарки

`POST avatar` (тело — картинка JPEG/PNG/WebP/GIF до 2 МБ; GIF хранится как есть и остаётся анимированной), `POST avatar/remove`, `GET avatar[?peer]` — своя или соседа (картинка), `GET avatars` — отпечатки: `{me, peers: {сосед: отпечаток}}`. Соседи узнают отпечаток из привета и забирают картинку один раз.

Пинг участника канала (панель): сигнал `{type: "ping"}` в комнату — у него мягкий звук и «… пингует».

## Прочее

- Настройки панели (звук, пинг, доска, вид) хранятся в мосте: `GET ui/settings`, `POST ui/settings {settings: {…}}` (`bridge-data/ui.json`, до 256 КБ).
- Панель раздаётся самим мостом: `/` и файлы `/app/*` (сборка `ui/dist`, вшита в программу).

`GET overview` (всё для панели одним запросом), `GET status`, `POST sync` (обменяться сейчас), `GET log?lines`, `GET changelog`, `POST open?target=…` (открыть папку в Проводнике).

## Инструменты для Claude (MCP)

`bridge mcp` — те же возможности как инструменты: `bridge_ui` (пульт: все действия выше), `bridge_board` (доска: смотреть, искать, класть, двигать, править, слои, фон, картинка доски), `bridge_screen` (снимок показа, пинг, линия, стереть), `bridge_api` (любой вызов), `bridge_peers`, `send_file`, `exchange_folder`, `agent_ask`, `agent_wait`, `agent_status`, `agent_stop`, `agent_threads`, и для ручного режима — `agent_wait_task`, `agent_inbox`, `agent_take`, `agent_progress`, `agent_report`. Строка подключения — в панели: «Агенты» → «Подключить к Claude».

## Сборки, большие файлы, план синхронизации, статус (3.3.1)

- `POST upload/part?id&offset` (тело — часть файла, по порядку) — большие файлы частями; затем обычный вызов (`mailbox/upload`, `fs/upload` и др.) с `?upload=<id>&size=<байт>` и пустым телом. Часть не по порядку — `409`.
- `GET builds` → `{history, received: [{peer, name, mode, files, bytes, at, target}], jobs, defaultRoot}`.
- `POST builds/send {peer, path, name, mode: patch|clean}` — упаковать папку (полный путь) и отправить; идёт в фоне (`jobs`). Архив уходит ручной отправкой в `_сборки/<name>/<mode>-<время>.zip`, получатель распаковывает его сам (`clean` — прежняя папка в корзину моста).
- `POST builds/target {peer, name, path}` — куда распаковывать сборку от соседа (пусто — `сборки/<name>`). `POST open?target=build&peer&name` — открыть папку сборки.
- `GET mounts/plan?peer&id` → `{items: [{path, act: download|update|delete_here|upload|delete_owner|restore|conflict, size, local, remote}], total, down, up, speedDown, speedUp, etaDown, etaUp (с; -1 — неизвестно), localFiles, remoteFiles, transfers, failed}` — что сделает обмен с папкой соседа.
- `POST tile/status {text?, color?}` — статус (текст до 255 знаков, цвет `#rrggbb`; пусто — убрать). `GET|POST tile/status/image[?peer]` — значок (до 256 КБ), `POST tile/status/image/remove`. Статус — в стиле плитки (`stText`, `stColor`), значок — `stImg` в `GET tile`.
