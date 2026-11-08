# Мост для Android

Приложение — полноправный участник сети мостов: свой ключ, свои соседи, связь через свой сервер (most-relay).
Внутри работает тот же узел моста на Rust (`mobile/` → `libmost_mobile.so`), интерфейс на Kotlin + Compose
ходит в его локальный API так же, как панель ПК. План: «Мост для Android — план» (Claude Docs).

## Что уже есть (заготовка, ветка 3.23)

- `mobile/` — ящик `most-mobile`: `start(dir, name)`, `stop()`, `status()`, `networkChanged()`, `version()`,
  `validName()`; привязки Kotlin — uniffi (`com.exerussus.most.core`). Тест: `cargo test --release -p most-mobile`.
- `android/` — первый запуск (имя телефона + приглашение `MRI-…` на сервер), служба переднего плана с узлом,
  экран «сервер + соседи».

Ещё нет: подключение соседа через сервер без IP (адрес `@сервер/участник`), чат, голос, файлы, привязка к ПК.

## Сборка на Windows (один раз — подготовка)

Свой набор для сборки (JDK 17, Gradle 8.11.1, Android SDK 35 и NDK 27) ставится одной командой в
`C:\Exerussus\Programs\Bridge\tools` — без Android Studio и Unity (около 2 ГБ):
```
.\build-android.cmd setup
```
Если своего набора нет, скрипт ищет Android Studio, затем Unity с модулем Android Build Support.

В PowerShell (`PS C:\…>`), один раз:
```
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
```

Ключ подписи (один раз, можно позже): `.\build-android.cmd keygen` — ключ ляжет в `C:\Exerussus\Programs\Bridge\keys`
(вне проекта и git). Сделайте копию этой папки: без ключа обновить приложение поверх нельзя. Без ключа сборка всё
равно пройдёт — получится отладочная версия.

Первая сборка скачает из интернета Gradle 8.11.1 и библиотеки (около 1 ГБ, один раз).

## Сборка

```
cd C:\Exerussus\Programs\Bridge\project
.\build-android.cmd
```

Итог — `publish\android\most-<версия>.apk`. На телефон — переслать файл и открыть (Android спросит разрешение
ставить приложения из этого источника).

## Первый запуск на телефоне

1. Приложение «Сервер» на ПК → «Пригласить» → скопировать строку `MRI-…` (5 минут).
2. На телефоне: имя (например `taranis-phone`) и эта строка → «Начать».
3. Строка «Свой сервер: на связи» — ядро, сервер и туннель работают.
