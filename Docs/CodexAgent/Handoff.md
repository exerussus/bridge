# Мост 3.24.0: Codex как агент-исполнитель — передача

2026-11-10. Основа: `Brief.md` (ред. 2), `TZ.md` (ред. 1).

## Что сделано

| Часть | Файлы |
|---|---|
| Настройки `engine`, `codex`, `codexSandbox`, `codexNetwork`, `codexModel`, `codexDirs` | `core/src/config.rs` |
| Hello: `agentEngine` | `core/src/proto.rs`, `node/src/http_peer.rs`, `node/src/link.rs`, `node/src/node.rs` (`PeerState.agent_engine`) |
| Нить: `engine`, `tokens`, `images`, `inputs`; `ask_with(…, images)`, `clean_inputs`; движок фиксируется в `enqueue`; смена движка у начатой нити → 409 | `node/src/agent.rs` |
| Исполнитель Codex: поиск, запуск, разбор `--json`, картинки, проверка | `node/src/agent/codex.rs` (новый модуль) |
| `GET agent/codex-check`, `GET agent/image?id&n`, `engine` в `GET agent` | `node/src/http_local.rs` |
| Инструменты MCP/efc: движок в `bridge_peers`, `images` в `agent_ask`, токены и картинки в `agent_wait`/`agent_status` | `node/src/tools.rs` |
| Панель: выбор движка, настройки Codex, «Проверить Codex», подписи «Кому», токены, превью и лента картинок | `ui/src/tabs/Agent.svelte`, `ui/src/lib/state.svelte.ts`, `ui/src/tabs/Help.svelte` |
| Тесты: подставной Codex, интеграционный тест | `node/tests/fixtures/fake_codex.rs`, `node/Cargo.toml`, `node/tests/net.rs` (`agent_codex_runs_images_and_continues`) |
| Версия, журнал, API | `Cargo.toml`, `ui/package.json`, `CHANGELOG.md`, `API.md` |

## Проверено (Linux, подставной Codex)

- `cargo test -p bridge-node --lib`: 14 тестов (в том числе разбор команды и аргументы `exec`/`resume`).
- `cargo test -p bridge-node --test net agent`: 10 тестов. Сюда входят прежние тесты Claude и новый тест Codex:
  - движок соседа виден через hello;
  - входная картинка передаётся через `-i`;
  - строки команд;
  - токены;
  - картинка, сохранённая Codex, учитывается без дубля;
  - продолжение в той же нити `resume`;
  - картинку, которую Codex не сохранил, мост кладёт сам;
  - ошибка хода.
- `cargo test -p bridge-core`, `svelte-check` (ошибок 0, предупреждений столько же, сколько было), `vite build`.

## Не проверено — приёмка на живом стенде (TZ п. 7)

- Настоящий `codex.exe` 0.167.0 на Windows, особенно продолжение: `codex exec --json … -s … --add-dir … resume <id> -`. Если `resume` не примет флаги, мост сам повторит запуск с минимальным набором: песочница и папки передаются через `-c sandbox_mode=…` и `-c sandbox_workspace_write.writable_roots=[…]`. Если не увидит поручение в stdin — передаст его аргументом. Что сработало, видно по событиям `info` в нити.
- Пути с кириллицей (`C:\Bridge\общие-мои\…`) в `--add-dir` и `-i`.
- `kill_tree` на «Стоп» — проверить, что дочерний `powershell.exe` тоже снимается.
- Превью картинок у заказчика появляется, когда обменная папка досинхронизировалась. До этого показывается «ждёт синхронизации».

## Ограничения

- Codex работает только в режиме `auto`. `agent_request` (вопрос людям) для Codex не подключён.
- Картинки, которые лежат вне обменной папки, видны только на исполнителе. В нити у них полный путь.
- Журнал сессий Codex (`rollout-*.jsonl`) мост не читает. Источник картинок — папка `generated_images/<thread_id>/` (или `CODEX_HOME`).
