# Изменения

## 3.7.0
- `client.Identity.Get()` — ключ пользователя для издателя приложения (`MostIdentityInfo`: Key, Fingerprint, Name, Publisher, Verified, Rotated).
- `client.Identity.Sign(purpose, payload)` — подпись строки `most-sign/v1` (мост подставляет app, pub, purpose, ts) → `MostSignature` (Key, Sig, Signed). Право `identity` в app.json.
- `MostMember.Key` — ключ участника сеанса (если у приложения есть право identity).
- WebGL: те же вызовы через панель моста.

## 3.5.0
- Первый выпуск. Связь с мостом: свои окна по `MOST_API`/`MOST_TOKEN`, редактор — «Мост → Подключение», WebGL — через панель моста.
- Сеансы в голосовом канале: `JoinOrCreate`, `Create`, `Join`, `List`, `Leave`, `Kick`.
- Данные: `Send` — надёжно (до 1 МБ), быстро и «последнее по ключу» (до 1200 байт); адреса `Host`, `All`, `Others`, cid.
- Общее состояние (`Set`/`Merge`/`Delete` по пути), присутствие, часы сеанса, задержки.
- Свой сервер: `SetConnect`, билеты участников, `CheckTicket`.
- Хранилище, уведомления, журнал активности.
- Имитация сети в редакторе; сборка приложения в папку моста (своё окно и WebGL).
