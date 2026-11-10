# Мост: Codex как агент-исполнитель — бриф

Ред. 2, 2026-11-10. Статус: **согласован**, ТЗ — `TZ.md`. Проект — `project/` (Мост 3.23.0, Rust).

## 1. Зачем

Исполнитель моста сейчас умеет запускать только Claude Code (`claude -p`). Нужен второй движок — **Codex** (OpenAI, `codex exec`):

- у Codex есть встроенная генерация картинок (Images 2.6, модели `gpt-image-2.6-*`), которая работает по логину ChatGPT — **без ключа API** и через собственное соединение Codex, в обход сетевой песочницы его команд;
- первая задача — арт персонажей Rig2D (Orven): Claude-оркестратор (в т. ч. в облаке, без сети до ПК — через файловый канал efc/1) поручает Codex атомарные задачи «сгенерируй N картинок по заявке, сохрани туда-то», сам оценивает, режет, проверяет и показывает результат на стендах Unity.

Поручение Codex идёт тем же путём, что и Claude: нить, ход в реальном времени, итог, продолжение в той же сессии, стоп, предел времени, канал efc для агента без сети. Для вызывающего разница — только в движке исполнителя.

## 2. Как сейчас (agent.rs)

- `AgentConfig`: `mode` (off/auto/manual), `allowFrom`, `workDir`, `claude`, `permissionMode`, `maxMinutes`, `maxParallel`, `maxBudgetUsd`, `extraArgs`, `exchange`, `fileChannel`.
- `run_claude` → `run_once`: процесс с поручением через stdin, разбор stdout построчно (`handle_line`, формат stream-json Claude: `system/init` → сессия, `assistant` → текст/инструменты, `result` → итог), stderr — в хвост ошибки, предел времени, `kill_tree`, продолжение через `--resume <session>`.
- Подсказка исполнителю (`context_prompt` + `finish_prompt`) — через `--append-system-prompt`; инструмент `agent_request` — через `--mcp-config`.
- Канал efc «agent-channel» отдаёт инструменты `bridge mcp` (в т. ч. `agent_ask {peer:"local"}`, `agent_wait`) агенту без сети.

## 3. Предложение

### 3.1 Настройки

В `AgentConfig` (все — с умолчаниями, старый `bridge.json` читается как раньше):

| Поле | Смысл | По умолчанию |
|---|---|---|
| `engine` | `claude` \| `codex` — чем исполнять поручения в режиме auto | `claude` |
| `codex` | путь к `codex` (`codex.exe` / `codex.cmd`); пусто — найти (PATH, `%APPDATA%\npm`, `%LOCALAPPDATA%\Programs`) | пусто |
| `codexSandbox` | `read-only` \| `workspace-write` \| `danger-full-access` | `workspace-write` |
| `codexNetwork` | разрешить сеть командам Codex (`-c sandbox_workspace_write.network_access=true`) | `false` |
| `codexModel` | модель Codex (`-m`), пусто — по его настройке | пусто |

Один движок на исполнителя (на NoteBook стоит только Codex). Выбора движка в поручении нет; сосед сообщает свой движок в hello (`agentEngine`), отправитель видит его в «Кому». У нити — поле `engine`; продолжение нити — тем же движком.

### 3.2 Запуск

```
codex exec --json --skip-git-repo-check -s <sandbox> -C <workDir> [--add-dir <out>…] [-i <img>…] [-m <model>] [-c …] [extraArgs] -
codex exec resume <thread_id> --json … -          # продолжение нити
```

- Поручение — через stdin (`-`), как у Claude. Подсказки исполнителю (`context_prompt` + `finish_prompt`) Codex не умеет брать отдельным параметром — **приклеиваются в начало поручения** блоком «Положение исполнителя».
- Инструмент `agent_request` (вопрос людям) — через MCP-сервер моста: `-c mcp_servers.most.command=<bridge.exe> -c mcp_servers.most.args=["mcp","--config",<bridge.json>]`. Если Codex не примет — без него (как со старым Claude без `--permission-prompts`).
- `codex.cmd` (установка через npm) запускается через `cmd /C` — иначе Windows не стартует `.cmd`.
- Предел времени, стоп, `kill_tree`, `maxParallel` — общие с Claude.

### 3.3 Разбор хода (`--json`, JSON Lines)

| Событие Codex | Нить моста |
|---|---|
| `thread.started {thread_id}` | `session = thread_id` (для `resume`) |
| `item.completed`, `item.type = agent_message` | `text` (+ «Этап:/Ожидание:» → фаза, как у Claude); последний — кандидат в итог |
| `item.started/completed`, `command_execution` | `tool`: «Команда: <command>» (+ код выхода, если ошибка) |
| `file_change` | `tool`: «Файлы: <пути>» |
| `mcp_tool_call` | `tool`: «<server>.<tool>» |
| `web_search` | `tool`: «Поиск: <запрос>» |
| (в `--json` картинок нет) | после хода: новые файлы `generated_images\<thread_id>\` → `tool`: «Картинка: <путь>», `images` нити |
| `reasoning`, `todo_list` / план | не пишутся (план — в фазу, если есть короткая строка) |
| `turn.completed {usage}` | `status = idle`, `result` = последний `agent_message`, `tokens` += usage |
| `turn.failed`, `error` | `status = failed`, `error` = сообщение |
| процесс кончился без `turn.completed` | как у Claude: «Codex завершился без итога» + хвост stderr; подсказка про вход (`codex login`), если в stderr есть login/auth |

Стоимость в долларах Codex не сообщает — у нити вместо `cost` копятся `tokens` (вход / выход / из кэша); панель показывает их. Сырые строки хода пишутся в `bridge-data/agent/<нить>.codex.jsonl` (для разбора новых типов событий; обрезка — по `keepHistoryDays`).

### 3.4 Картинки (подтверждено пробой, Codex 0.167.0)

Факты:
- Встроенная генерация работает в `codex exec` под входом ChatGPT, без ключа API (~35–40 с на картинку).
- Codex зовёт `tools.image_gen__imagegen({prompt, transparent_background})`; файл сам ложится в `%USERPROFILE%\.codex\generated_images\<thread_id>\exec-<uuid>.png`, затем Codex копирует его командой (`Copy-Item`) туда, куда сказано.
- В потоке `--json` события картинки **нет** (только `agent_message`, `command_execution`, `turn.completed`). Оно есть только в журнале сессии `.codex\sessions\…\rollout-*.jsonl`: `item_completed`, `item.type = "Extension"`, `kind = "image_gen.generation"`, поле `savedPath` (+ `revisedPrompt`, `transparentBackground`, `failure`).
- Песочница `workspace-write` не пускает запись вне `-C`; с `--add-dir <папка>` — пускает.
- `-i/--image <FILE>...` жадный: съедает промпт. Промпт только через stdin (`-`) или после `--`.
- Параметры инструмента (t5): `prompt`, `transparent_background`, `referenced_image_paths[]` (правка по локальным файлам), `num_last_images_to_include` (≤5). Размера/пропорций/качества нет — формат берётся из входной картинки: квадратный промпт без входа → 1254×1254 RGB; правка по `pose.png` 1024×1536 → 1024×1536 RGBA.
- t4 (правка по позе + референс The Quest, 95 с): прозрачный фон настоящий (78% пикселей альфа 0), силуэт совпал с манекеном до ~10 px (bbox 278–743×149–1441 против 286–737×158–1438), ноги на линии земли, ключевые цвета соблюдены.
- **Самооценке Codex верить нельзя:** в итоге он написал «фон — тёмная виньетка, ноги не на линии» — оба утверждения ложны. Оценку делает только `rigart score`/мост, не исполнитель.

Порядок для моста:
1. Подсказка исполнителю (`engine = codex`): «картинки — встроенным инструментом; каждую сохраняй в путь из поручения; в итоге строки `IMAGE: <путь>`».
2. Папки вывода из поручения/настроек передаются как `--add-dir`.
3. После хода мост собирает `IMAGE:` и **сверяет с источником истины** — `generated_images\<thread_id>\*.png` (thread_id из `thread.started`). Картинки, которые Codex не скопировал, мост кладёт сам в `workDir\images\<нить>\`. Журнал rollout не читается (внутренний формат, может поменяться) — только как запасной путь для `revisedPrompt` в отладке.

### 3.5 Проба (сделана 2026-11-10)

`share\input\codex-probe\` → `share\output\codex-probe\`: t1 (в рабочую папку) — ок; t2 (вне) — отказ песочницы; t3 (`--add-dir`) — ок; t4 — сначала упал на синтаксисе `-i`; повтор (`run-t4.cmd`, после `--`) — успех; t5 — параметры инструмента.

### 3.6 Оркестрация из облака (efc)

Ничего нового в канале не нужно: `agent_ask {peer, prompt, images?}` → `agent_wait` / `agent_status` уже есть в «agent-channel». Добавляются только `images` (входные картинки) в аргументах `agent_ask` и `images` в ответе `agent_wait/agent_status`. Включение — как сейчас: «Агенты» → «канал для агента без сети».

### 3.7 Панель

«Агенты» → исполнитель: переключатель «Claude / Codex», для Codex — путь, песочница, сеть, модель; «Проверить Codex» (`codex --version` + вошёл ли: `codex login status`). В нити — движок, токены, список картинок с превью.

## 4. Вне рамок

- Codex как оркестратор (MCP моста для Codex-сессий людей) — отдельно, если понадобится.
- Режим manual для Codex (сессия Codex сама берёт поручения) — не нужен для генерации.
- Ограничение расходов Codex в деньгах — Codex их не сообщает; есть предел времени.

## 5. Решения (бывшие вопросы)

1. Песочница `workspace-write`, сеть выключена (`codexNetwork: false`).
2. Рабочая папка — общая `workDir` моста; обменная папка нити и `codexDirs` — через `--add-dir`; путь результата — в поручении.
3. Версия 3.24.0, CHANGELOG и API.md; документы — `Docs/CodexAgent/` (Brief, TZ, Handoff).
4. Один движок на ПК (2026-11-10: на Taranis NoteBook только Codex).
