# Этап 2: вход по коду устройства и хранение токена

Спецификация от `api-scout` (2026-11-04). Сведения из MarshalX (LGPL),
Cassette (GPL) и ya-passport-auth пересказаны своими словами, код оттуда не
переносится. Пункты с пометкой «проверить» не подтверждены вживую.

## Главное

1. Поток кода устройства с music client_id заявлен рабочим только
   мейнтейнером MarshalX («для большинства аккаунтов»). Проверить первым делом.
2. Запасной путь без ввода пароля в приложении — поток кода устройства с
   client_id Passport и обмен x_token на music-токен (см. «Запасные варианты»).
3. `keyring` с версии 4 разделён. Автор советует приложениям `keyring-core` и
   отдельные крейты-хранилища.
4. `yandex-music` тянет reqwest с rustls и aws-lc-sys. На Windows x86_64 MSVC
   aws-lc-sys нужен NASM либо его готовые объекты.
5. `account/status` без токена отвечает 200, только без `uid`. Для проверки
   токена статуса 200 мало.

## OAuth Яндекс ID

Документация: https://yandex.com/dev/id/doc/ru/codes/screen-code-oauth.md
(зеркало yandex.ru/dev/id без капчи).

### `POST https://oauth.yandex.ru/device/code`

- `Content-Type: application/x-www-form-urlencoded`, параметры в теле.
- Обязательный: `client_id`. client_secret на этом шаге не нужен.
- Необязательные:
  - `device_id` — 6–50 печатных ASCII-символов. Генерируем UUID один раз и
    храним в конфиге: это не секрет. С ним повторный вход заменяет токен, а не
    копит новые (не больше 30 токенов устройств на пользователя, лишние
    вытесняются), и токен можно отозвать.
  - `device_name` — до 100 символов, показывается на странице «Доступы к
    данным». Без `device_id` игнорируется.
- Ответ 200:
  ```json
  {"device_code":"3e2a…848c","user_code":"h5nbcr6c",
   "verification_url":"https://oauth.yandex.ru/device","interval":5,"expires_in":300}
  ```
  `verification_url` показываем из ответа, но только с https и хостом из списка:
  `oauth.yandex.ru`, `ya.ru`, `yandex.ru`.
- Ошибка: `{"error":"…","error_description":"…"}`. Коды: `invalid_client`,
  `invalid_request`, `unauthorized_client`, `Basic auth required`,
  `Malformed Authorization header`.

### `POST https://oauth.yandex.ru/token` — опрос

- Тело: `grant_type=device_code`, `code=<device_code>`, `client_id`,
  `client_secret`. Можно и через `Authorization: Basic`, но шлём в теле.
- Ответ 200:
  `{"token_type":"bearer","access_token":"…","expires_in":…,"refresh_token":"1:…","scope":"…"}`.
- Ошибки приходят как HTTP 400 с JSON. Разбираем тело, а не вызываем
  `error_for_status()` заранее. Код ошибки может содержать пробелы.
- Первый опрос — через `interval`, не чаще `interval`. Предел ожидания — по
  `expires_in`. Ожидание отменяется по Ctrl+C.

| Код | Реакция |
|---|---|
| `authorization_pending` | Ждать `interval` и опрашивать снова |
| `slow_down` (RFC 8628, в документации Яндекса нет) | Увеличить интервал на 5 с и продолжать |
| `invalid_grant`, `expired_token` | Остановить, «Код истёк», предложить начать заново |
| `access_denied` | «Доступ отклонён», без автоповтора |
| `invalid_scope` | Начать заново |
| `invalid_client`, `unauthorized_client` | «Яндекс отклонил приложение», предложить запасной способ |
| `invalid_request`, `unsupported_grant_type`, `bad_verification_code`, `Basic auth required`, `Malformed Authorization header` | Ошибка программы: общее сообщение, подробности в debug-лог без токенов |
| Сеть, 5xx | Повторить на следующем тике, после 3 подряд прервать |

### Время жизни, обновление, отзыв

- Срок жизни music-токена — около года (по примерам, проверить).
  `refresh_token` выдаётся, срок у него тот же.
- Обновление: `POST /token`, тело `grant_type=refresh_token`, `refresh_token`,
  `client_id`, `client_secret`. Приходит новая пара. Считаем refresh_token
  одноразовым: новую пару сразу пишем в keyring. Яндекс советует обновлять
  долгоживущие токены раз в три месяца.
- Токен перестаёт работать, если пользователь отозвал доступ, сменил пароль,
  переключил 2FA, нажал «выйти везде», если вышел срок или приложение
  заблокировано.
- Отзыв при выходе: `POST https://oauth.yandex.ru/revoke_token`, тело
  `access_token`, `client_id`, `client_secret`. Ответ 200 `{"status":"ok"}`,
  в том числе для уже недействительного токена. Без `device_id` токен не
  отзывается (400 `unsupported_token_type`). При выходе обязательно удаляем
  запись из keyring, отзыв делаем по возможности, его сбой выходу не мешает.
- В крейте `yandex-music` OAuth нет, пишем сами в `auth`: три
  form-urlencoded-запроса через reqwest.

## Учётные данные и заголовки

| | OAuth client_id и client_secret | `X-Yandex-Music-Client` |
|---|---|---|
| Для чего | Получение и обновление токена на `oauth.yandex.ru` | Идентификация клиента на `api.music.yandex.net` |
| Значение | `23cabbbdc6cd418abb4b39c32c41195d` / `53bc75238f0c4d08a118e51fe9203300` | `YandexMusicAndroid/24023621` |
| В `yandex-music` | Нет | `ClientBuilder::client_id(..)`, `DEFAULT_CLIENT_ID` |

- Это публичные константы Android-приложения, их используют все известные
  неофициальные клиенты. Чужие учётные данные могут противоречить условиям
  Яндекса, вплоть до блокировки.
- Своё OAuth-приложение не подходит: по данным MarshalX (2022), токен даёт
  `ownerOtherwiseUserBindingError`.

### Запасные варианты (пароль вводится только на страницах Яндекса)

- **A. Код устройства через Passport.** `device/code` с client_id Passport
  `c0ebe342af7d48fbbbfcf2d2eedb8f9e`, опрос `/token` с его секретом, получаем
  x_token. Затем `POST https://oauth.mobile.yandex.net/1/token`, тело
  `grant_type=x-token`, `access_token=<x_token>`, music `client_id` и
  `client_secret`, получаем music-токен. Подтверждено кодом YandexStation и
  e2e-проверкой ya-passport-auth (2026-04). Минус: два набора чужих данных.
- **B. Implicit в браузере.**
  `https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d`.
  Пользователь копирует токен из адресной строки `music.yandex.ru#access_token=…`.
  Без refresh_token, неудобно.
- **C. Встроенный веб-вью.** Для iced тяжело, за рамками этапа 2.

## Проверка токена: `GET https://api.music.yandex.net/account/status`

- Заголовки: `Authorization: OAuth <token>` (не Bearer),
  `X-Yandex-Music-Client: YandexMusicAndroid/24023621`.
- Без токена: 200, `result.account` без `uid`, `login`, `plus`.
- Фальшивый токен: 401, тело в стиле Spring
  `{"timestamp":…,"status":401,"error":"Unauthorized",…}`, без `invocationInfo`.
  В 2023 году для истёкшего токена приходило `{"name":"session-expired",…}`.
- **Токен рабочий**, если ответ 200 и в `result.account` есть `uid`.
  401 и 403 — токен недействителен, ведём на повторный вход. Остальное —
  временный сбой, токен не удаляем.
- Подписка (этап 3): `result.plus.hasPlus`. Ещё есть `serviceAvailable` и
  `regionCode`: при недоступности в регионе нужно отдельное сообщение.
- `YandexMusicClient::get_account_status()` на анонимном ответе даёт
  `JsonParseError` (обязательные `child`, `plus`, `permissions`), на 401 —
  `YandexMusicError { name: "RequestFailed" }`, потому что нет
  `invocationInfo`. Для этапа 2 делаем свой GET через `reqwest` и смотрим
  `StatusCode`. Метод крейта — на этапе 3.

## Крейт `yandex-music` 0.7.0

- `YandexMusicClient::builder(token).build()?` ставит `Authorization` и
  `X-Yandex-Music-Client`. Таймаутов и User-Agent не ставит, заголовок
  авторизации не помечен `sensitive`.
- Свой клиент: `.custom_client(reqwest::Client)`. Тогда оба заголовка ставим
  сами: `Authorization` с `HeaderValue::set_sensitive(true)`, плюс таймауты.
  Для аудио на этапе 5 — отдельный клиент без общего таймаута.
- reqwest `0.13.2` с `form`, `json`, без отключения default-features. Наш
  `reqwest = "0.13"` сольётся с ним в одну копию. `HTTPS_PROXY` учитывается
  через `system-proxy`.
- Паники внутри крейта, которые наши линты не ловят: `unwrap` в
  `api/utils.rs`, `model/utils.rs:25`, `model/info/download_info.rs:34`
  (этап 5).

### Сборка TLS

- rustls с провайдером aws-lc-rs и rustls-platform-verifier. OpenSSL не нужен.
  Убрать aws-lc-sys нельзя: features аддитивны.
- Windows MSVC x86_64: компилятор C есть, NASM нет. Варианты: поставить NASM,
  `AWS_LC_SYS_PREBUILT_NASM=1` или feature `prebuilt-nasm` у `aws-lc-rs`.
  `AWS_LC_SYS_NO_ASM=1` — только для debug. CMake и Go нужны только для FIPS.
- Arch/CachyOS: хватает `base-devel`. Корневые сертификаты — из
  `ca-certificates`.
- Лицензии aws-lc-sys, rustls, reqwest — permissive.

## Хранилище секретов

- `keyring` 4.2.0 (2026-08-29) — обёртка с режимом совместимости `v1`
  (по умолчанию). Нужна версия не ниже 4.1.4: 4.1.3 отозвана.
- Рекомендация автора для приложений: `keyring-core` 1.0.1 плюс хранилища:
  - Windows: `windows-native-keyring-store` 1.1.0;
  - Linux: `zbus-secret-service-keyring-store` 1.0.1 с `crypto-rust`
    (чистый Rust, libdbus не нужна).

  Один вызов `keyring_core::set_default_store(Store::new()?)` при старте:
  ошибка хранилища видна сразу. Для тестов и отладочных хуков есть
  `keyring_core::mock::Store`. На Windows можно выбрать `persistence=Local`
  (по умолчанию `Enterprise`).
- Все крейты под MIT OR Apache-2.0.
- Windows: секрет до 2560 байт в UTF-16, то есть до 1280 символов. JSON с
  access_token, refresh_token и сроком умещается. Больше — `Error::TooLong`.
- Linux: нужен запущенный провайдер Secret Service: gnome-keyring, KeePassXC
  или KWallet с включённым Secret Service. Без него `PlatformFailure`,
  заблокированная коллекция — `NoStorageAccess`. На файл не откатываемся
  (правило 5), показываем понятное сообщение.
- Нет записи — `Error::NoEntry`, при выходе это успех. `Error` помечен
  `#[non_exhaustive]`.
- Вызовы блокирующие: из tokio — через `spawn_blocking`.
- Пакет `dbus` для keyring не нужен (он для souvlaki на этапе 9).

## Проверить вживую на этапе 2

1. `device/code` с music client_id: коды или `invalid_client`.
2. Опрос `/token`: нужен ли `client_secret`, статус `authorization_pending`,
   приходит ли `slow_down`, что на просроченный код, реальный `expires_in`.
3. После подтверждения: длина `access_token`, `expires_in`, `refresh_token`,
   есть ли `uid` в `account/status`.
4. `grant_type=refresh_token`: работает ли, меняется ли refresh_token.
5. Отозванный токен: что отвечает `account/status`.
6. Без `X-Yandex-Music-Client` и без User-Agent.
7. Первая сборка на Windows: aws-lc-sys без NASM и с готовыми объектами,
   `cc` с Visual Studio 2026.
8. CachyOS: сборка с `base-devel`, запуск без провайдера Secret Service и с
   KWallet.
9. Запись видна в Credential Manager (`cmdkey /list`) и удаляется при выходе.
10. `device_name` на странице «Доступы к данным», повторный вход не плодит
    записи.
11. «Нет сети» через `HTTPS_PROXY=http://127.0.0.1:9`, понятное сообщение.
12. `serviceAvailable` и `regionCode`.

## Расхождения между источниками

- Срок жизни кодов: пример — 300 с, текст — 10 минут.
- `bad_verification_code` описан как «не 7-значное число», а пример
  `device_code` — 32 hex-символа.
- RFC-коды (`slow_down`, `expired_token`, `access_denied`) в документации
  Яндекса не описаны, но клиенты их разбирают.
- Страница ввода кода: `oauth.yandex.ru/device` или `ya.ru/device`. Показываем
  `verification_url` из ответа.
- MarshalX делает поток кода устройства с music-клиентом, ya-passport-auth — с
  Passport-клиентом и обменом.
- Ответ на недействительный токен: `session-expired` (2023) против 401 Spring
  (2026).
- В OpenAPI `uid` и `login` обязательны, на деле их может не быть.
- Длину токена не проверять: формат менялся.
