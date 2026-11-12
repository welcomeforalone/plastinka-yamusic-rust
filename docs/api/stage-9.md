# Этап 9: тексты песен, LRCLib, LRC, настроения волны, souvlaki — спецификация API

Разведка — 2026-11-10. Источники: исходники крейта `yandex-music` 0.7.0 (MIT)
— `src/api/track/get_lyrics.rs`, `src/model/info/lyrics.rs`,
`src/api/utils.rs`, `src/model/track/mod.rs`, `src/model/track/supplement.rs`,
`src/api/rotor/*`, `src/model/rotor/*`; исходники `souvlaki` 0.9.3 (MIT,
скачаны `cargo info souvlaki` в реестр, в `Cargo.toml` проекта не добавлялись);
наш код `src/api/stream.rs`, `src/api/rotor.rs`, `src/ui/mod.rs`.

У агента нет доступа к вебу и настоящему токену, живых запросов не было.
Пометки:

- **[крейт]** — прочитано в исходниках крейта, достоверно;
- **[souvlaki]** — прочитано в исходниках souvlaki 0.9.3, достоверно;
- **[НЕ ПРОВЕРЕНО, по памяти]** — общие знания о сервисах, лид должен сверить
  (WebFetch/WebSearch) или проверить вживую.

Код MarshalX и чужих клиентов не переносим. Заголовки у запросов к
`api.music.yandex.net` те же, что везде (`Authorization: OAuth <token>`,
`X-Yandex-Music-Client` из `MUSIC_CLIENT`). Звук и аудио в этом этапе не
затрагиваются: тексты — метаданные (правило 1 их не касается).

## Решения (пользователь, 2026-11-10) и сверка лида

1. Подключаем `souvlaki` 0.9.3 с `default-features = false, features =
   ["use_zbus"]`. Лид проверил: `zbus` 5.19 в Linux-сборке уже с `async-io`
   (`mundy`, `secret-service`), без `tokio` — с `pollster` в souvlaki совместим.
2. Тексты кэшируем на диске (метаданные, как обложки).
3. LRCLib включён по умолчанию (переключатель в настройках).
4. Меню таблетки на экране волны — как в официальном клиенте: Занятие,
   Характер, Настроение, Язык.
5. Настройки волны — только seeds новой сессии (раздел 4), без `settings2`/
   `settings3`, записи в аккаунт нет.

## 1. Тексты песен Яндекса

### Признаки наличия текста в треке

В объекте трека (`/tracks`, «Мне нравится», альбомы, плейлисты, волна):

```json
"lyricsAvailable": true,
"lyricsInfo": {"hasAvailableSyncLyrics": true, "hasAvailableTextLyrics": true}
```

- **[крейт]** `Track.lyrics_available: Option<bool>`,
  `Track.lyrics_info: Option<TrackLyricsInfo { has_available_sync_lyrics: bool,
  has_available_text_lyrics: bool }>`. У альбома тоже `lyricsAvailable`.
  Оба поля необязательные.
- Наш `parse_track` (`src/api/tracks.rs`) этих полей пока не читает: нужно
  добавить в `api::Track` (например, `lyrics: LyricsAvailability { synced,
  text }`) мягким разбором: нет `lyricsInfo` → брать `lyricsAvailable`
  (как «текст есть, формат неизвестен»), нет и его → «неизвестно».
- Как использовать: `hasAvailableSyncLyrics` → просить `format=LRC`;
  только `hasAvailableTextLyrics` → `format=TEXT`. Флаги могут отсутствовать
  или врать (**[НЕ ПРОВЕРЕНО]**): если флаги не пришли, пробуем `LRC`, на ошибку
  или пустой ответ — `TEXT`, потом запасной источник (LRCLib, раздел 2).
- Запасной источник текста **[крейт]**: `GET /tracks/{id}/supplement`
  (`get_track_supplement`) → `lyrics: [{id, lyrics, fullLyrics, hasRights,
  textLanguage, showTranslation}]`. Это уже готовый обычный текст в JSON,
  без подписи. Нужно ли — решает лид; в этапе по умолчанию не используем
  (**[НЕ ПРОВЕРЕНО]**, отдаёт ли он текст без `hasRights`).

### `GET /tracks/{id}/lyrics` — адрес на файл текста

**[крейт]** (`get_lyrics.rs`): `GET https://api.music.yandex.net/tracks/{trackId}/lyrics`

| Параметр | Значение |
|---|---|
| `format` | `LRC` (с метками времени) или `TEXT` (обычный) |
| `timeStamp` | время, секунды Unix |
| `sign` | подпись, см. ниже |

`trackId` — только число, без `:albumId` (в крейте `track_id` идёт в адрес как
есть; у нас числовые id уже проверяет `plausible_id`).

**Подпись [крейт]** (`utils.rs`, `create_sign`):

1. ключ HMAC-SHA256 — `p93jhgh689SBReK6ghtw62` (`SIGN_KEY`, тот же, что у
   `get-file-info`; у нас константа `FILE_INFO_KEY` в `src/api/stream.rs`);
2. сообщение — `trackId` + `timestamp` **без разделителей и в порядке «id,
   потом время»**: `format!("{track_id}{timestamp}")`. У `get-file-info` порядок
   обратный (`ts` первым) и там много полей — не путать;
3. результат — base64 стандартного алфавита **целиком, с `=` на конце**
   (у `get-file-info` последний символ отрезается, здесь нет);
4. значение кодируется для URL по байтам: не кодируются только
   `A–Z a–z 0–9 - _ . ~`, остальное — `%XX` (заглавными). То есть `+` → `%2B`,
   `/` → `%2F`, `=` → `%3D`. Крейт вклеивает уже закодированный `sign` прямо в
   строку адреса. У нас безопаснее то же: собрать строку запроса вручную, а
   не отдавать `sign` в `query()` reqwest (иначе будет двойное кодирование).

**Переиспользование.** Ключ и HMAC-обвязка (`Hmac::<Sha256>`, `STANDARD.encode`)
есть в `src/api/stream.rs` (`file_info_sign`), но сама функция заточена под
формат `get-file-info` (отрезает `=`, другой порядок). Нужна **отдельная
функция** `lyrics_sign(track_id, ts) -> String` в новом модуле
`src/api/lyrics.rs` с общей константой ключа (вынести `FILE_INFO_KEY` в
`api/mod.rs` как `SIGN_KEY`, либо `pub(super)` из `stream.rs`). Зависимости
(`hmac`, `sha2`, `base64`) уже в проекте. Эталон для теста посчитать отдельно
в Python (как у `get-file-info`), например `trackId=12345`, `ts=1700000000`.

**Ответ [крейт]** (`TrackLyrics`, поля camelCase, в обёртке `result`):

```json
{"result":{
  "downloadUrl":"https://…",
  "lyricId":12345678,
  "externalLyricId":"abcd…",
  "writers":["Автор слов","Автор музыки"],
  "major":{"id":1,"name":"Правообладатель"}}}
```

Значения придуманы по смыслу, структура — по крейту. Крейт требует все пять
полей, у нас разбор мягкий: нужен только `downloadUrl`; `writers` и
`major.name` — по желанию (подпись «Автор текста», правообладатель). Нет
`downloadUrl` → «текста нет».

**Ошибки и крайние случаи** (**[НЕ ПРОВЕРЕНО, по памяти]**):

- у трека нет текста — скорее всего HTTP 404 (или 200 с `error`/пустым
  `result`); любой не-2xx считаем «текста нет», не «ошибка сети»;
- неверная подпись — 400/403 (проверить вживую, это главный риск: порядок
  полей в сообщении взят из крейта и не сверен с живым ответом);
- 401 — «вход устарел», как у остальных запросов;
- `downloadUrl` подписан и живёт недолго (как ссылки на звук): не кэшировать
  сам адрес, только текст.

### Загрузка текста по `downloadUrl`

**[НЕ ПРОВЕРЕНО, по памяти]** (крейт этого шага не делает):

- обычный `GET` по ссылке, тело — обычный текст UTF-8: для `LRC` строки
  вида `[мм:сс.хх] текст`, для `TEXT` — строки с переводами строк;
- хост — хранилище Яндекса (ожидается `*.yandex.net`/`*.yandex.ru`); ссылку
  принимаем только `https` и только на этих хостах, как `allowed_url` для
  звука (`src/api/stream.rs`);
- **токен в этот запрос не отправляем** (чужой/CDN-хост; правило токена).
  Нужна ли авторизация для самого файла — проверить вживую (ожидаем: нет,
  ссылка подписана сама);
- лимит размера тела (например, 256 КБ) и таймаут — на нашей стороне;
- кодировка: возможны BOM и `\r\n`; разбор их терпит (раздел 3).

### Покрытие крейтом

`get_lyrics` в 0.7 есть, но, как и в остальных местах (решение этапа 4), крейт
не подключаем: запрос свой, через наш `reqwest::Client` с прокси и мягким
разбором. Подписи и модель совпадают с крейтом (кроме мягкости полей).

## 2. LRCLib

Сверено лидом по официальной документации (`lrclib.net/docs.md`),
2026-11-10. Сервис бесплатный и без регистрации; тексты вносит сообщество,
лицензия на тексты не оговорена — внешний источник, не наш звук (правило 7 к
тексту не относится). Запасной источник, включён по умолчанию
(`lyrics_lrclib` в `config.toml`, решение пользователя).

### `GET https://lrclib.net/api/get`

| Параметр | Значение |
|---|---|
| `track_name` | название трека (обязателен) |
| `artist_name` | исполнитель (обязателен) |
| `album_name` | альбом (рекомендуется; без него не шлём) |
| `duration` | длительность, целые секунды, 1–3600 (рекомендуется) |

Совпадение только при разнице длительности не больше ±2 с. Заголовки:
`User-Agent` (у нас общий клиент, `Plastinka/<версия>`; документация просит
имя, версию и ссылку на страницу или контакт — домашней страницы у проекта
нет, поэтому без ссылки и без личных данных), авторизации нет.

200:

```json
{"id":3396226,"trackName":"…","artistName":"…","albumName":"…",
 "duration":233,"instrumental":false,
 "plainLyrics":"строка 1
строка 2
…",
 "syncedLyrics":"[00:17.12] строка 1
[00:20.30] строка 2
…"}
```

- `syncedLyrics` — LRC (раздел 3) или `null`; `plainLyrics` — обычный текст
  или `null`;
- `instrumental: true` — трек без слов, оба текста `null`: «Без слов», а не
  «текста нет»;
- оба текста `null` и не инструментал — текста нет.

**404** — JSON `TrackNotFound`: штатное «текста нет», не ошибка. **429**
(лимит около 90 запросов в минуту, есть `Retry-After`) — не повторять сразу,
показать «Текст недоступен: сервис LRCLib просит подождать». 5xx и сеть —
понятное сообщение, воспроизведение не страдает.

### `GET https://lrclib.net/api/search`

`q` либо `track_name`, плюс `artist_name`, `album_name`. Ответ — массив
записей того же вида (до 20). Запасной ход при 404 от `/api/get`: ищем по
`track_name` + `artist_name` (альбом не шлём: он сужает поиск), берём запись
с длительностью в пределах ±2 с, текст с метками предпочтительнее, затем
ближайшая по длительности. Если у трека нет длительности или у записи её нет,
сверить нечем: запись с известной нам длительностью без её значения не берём.

### Прочее

- Яндекс первым; LRCLib — когда у Яндекса текста нет или он не получен.
- Результат кэшируется на диске отдельно для каждого источника (метаданные,
  как обложки): найденный текст — до года, «инструментал» — 30 дней,
  «текста нет» — 7 дней, ошибки не кэшируются. Каталог — `<кэш ОС>/lyrics`.
- Ищем по названию и исполнителям, как они в данных трека Яндекса.

## 3. Формат LRC

Формат неформальный; разбор пишем сами (без новой зависимости), мягкий:
битая строка пропускается, а не роняет текст. Что должен уметь разбор:

1. **Метка времени** `[мм:сс.хх]`: минуты — одна и более цифр (бывает и
   `[100:05.00]`, больше 99), секунды `00–59` (терпим и больше), дробная
   часть `.x`, `.xx`, `.xxx` (десятые, сотые, тысячные) или вовсе без неё;
   в части LRC-диалектов разделитель дроби — `:` (`[01:02:34]`) — терпим.
   Время = минуты·60 + секунды + дробь, храним в миллисекундах (`u64`).
2. **Строка** = одна или несколько меток подряд + текст: `[00:12.00][01:30.50]
   Припев` даёт две записи с одним текстом. Текст после последней метки,
   пробелы по краям обрезаются.
3. **Пустой текст** при метке (`[00:45.00]`) — не мусор, а пауза: запись
   остаётся (пустая строка в панели, снимает подсветку предыдущей). Строки
   вообще без меток и без метаданных пропускаются.
4. **Метаданные** — `[ключ:значение]`, где ключ не число: `[ar:Исполнитель]`,
   `[ti:Название]`, `[al:Альбом]`, `[au:Автор]`, `[length:03:45]`,
   `[by:…]`, `[offset:+500]`, `[re:…]`/`[ve:…]`. Различать от метки времени
   по тому, что **до двоеточия — цифры**. Пропускаем всё, кроме `offset`:
   `[offset:±мс]` — сдвиг всех меток в миллисекундах (по спецификации
   плюс — раньше; знак и применение проверить на живом тексте, по умолчанию
   игнорировать).
5. **Расширенный LRC** (посословный, `<00:12.00>слово`): если встретится,
   теги `<…>` вырезаем и показываем строку целиком. Яндекс отдаёт построчный
   LRC (**не проверено**).
6. Порядок: итоговый список сортируем по времени стабильной сортировкой
   (метки на одной строке и редкие несортированные файлы).
7. Окружающее: BOM (`\u{feff}`) в начале, `\r\n`, пробелы, пустые строки —
   терпим; чужой формат без единой метки времени считаем обычным текстом.
8. Подсветка текущей строки: индекс последней записи с `time ≤ позиция`
   (бинарный поиск по `partition_point`); без индексации и `unwrap`, как в
   правилах проекта.

Покрытие крейтом: готового парсера в `yandex-music` нет. Новую зависимость
под LRC не берём (формат тривиален).

## 4. Настроения волны (rotor)

Что уже сделано на этапе 8: `POST /rotor/session/new` c
`seeds:["user:onyourwave"]`, `…/tracks`, `…/feedback` (`src/api/rotor.rs`,
спека `docs/api/stage-8.md`). Здесь — как расширить до настроений и занятий.

### Каталог станций

**[крейт]** `get_stations_dashboard` — `GET rotor/stations/dashboard`,
ответ `{dashboardId, pumpkin, stations:[Rotor]}`; `get_all_stations` —
`GET rotor/stations/list` (опционально `?language=ru`, **в крейте так**, значение
по памяти — код языка), ответ `[Rotor]`; `get_station` — `GET
rotor/station/{type:tag}/info`, ответ `[Rotor]` (**у крейта `type:tag`
идёт в путь с двоеточием как есть**).

`Rotor` **[крейт]**:

```json
{"station":{
   "id":{"type":"mood","tag":"winter"},
   "name":"Зимнее",
   "icon":{"backgroundColor":"#…","imageUrl":"avatars.yandex.net/…/%%"},
   "mtsIcon":{…},"idForFrom":"mood-winter",
   "restrictions":{…},"restrictions2":{…},
   "fullImageUrl":"…","parentId":{"type":"…","tag":"…"}},
 "settings":{"language":"…","diversity":"…","mood":null,"moodEnergy":"…","energy":null},
 "settings2":{ … те же поля … },
 "adParams":{ … },"rupTitle":"…","rupDescription":"…"}
```

Нужное нам: `station.id.type` + `station.id.tag` (идентификатор станции
`type:tag`, он же `seed` для сессии), `station.name`, `station.icon`
(цвет и картинка таблетки), `station.idForFrom`, `settings2`, а также
`restrictions2` (что можно менять). Модель крейта строгая (много
обязательных полей: `adParams`, `mtsIcon`, …), разбор у нас **мягкий,
поэлементный**, как в этапе 8.

`restrictions`/`restrictions2` **[крейт]**:

```json
"restrictions2":{
  "language":{"type":"enum","name":"Язык","possibleValues":[{"name":"Любой","value":"any"},…]},
  "diversity":{"type":"enum","name":"Характер","possibleValues":[{"name":"…","value":"favorite"},…]},
  "moodEnergy":{"type":"enum","name":"Настроение","possibleValues":[…]},
  "mood":{"type":"scale","name":"…","min":{"name":"…","value":0},"max":{…}}}
```

(`mood` и `moodEnergy` необязательные.) **[НЕ ПРОВЕРЕНО, по памяти]** типичные
значения `settings2`: `diversity` — `favorite` (любимое), `popular`
(популярное), `discover` (незнакомое), `default`; `moodEnergy` — `all`, `active`
(бодрое), `fun` (весёлое), `calm` (спокойное), `sad` (грустное); `language` —
`any`, `russian`, `not-russian`, `without-words`. Брать значения и их русские
подписи **из `possibleValues` ответа**, а не зашивать в код.

Типы станций в `station.id.type` **[НЕ ПРОВЕРЕНО, по памяти]**: `user`
(`user:onyourwave` — «Моя волна»), `mood` (настроения), `activity` (занятия),
`epoch` (эпохи), `genre` (жанры), `local` (язык/регион), а также `track:{id}`,
`artist:{id}`, `playlist:{uid}_{kind}`. В `dashboard.stations` обычно
«Моя волна» и набор настроений/занятий/эпох/жанров; полный список — в
`stations/list`.

### Старт сессии с настроением или занятием

Тело то же, что в этапе 8, меняется только `seeds` **[крейт]** — это список
строк, у крейта нет ограничений:

```json
{"seeds":["mood:winter"],"queue":[],"includeTracksInResponse":true,
 "includeWaveModel":true,"interactive":true}
```

- Seed = `{type}:{tag}` станции из каталога (`mood:winter`, `activity:work-background`,
  `epoch:90s`, `genre:rock`, `user:onyourwave`). Точные теги брать **только из
  ответа `dashboard`/`list`** (мои примеры тегов — по памяти). Значение seed
  идёт в тело JSON, а не в адрес, но тег всё равно проверять
  (`[A-Za-z0-9_.:-]`, длина), как `safe_session`.
- Ответ `session/new` тот же, что у волны: `radioSessionId`, `batchId`,
  `sequence[]`, `wave{name, idForFrom, seeds, stationId}`. Поле `from` в
  событиях — `wave.idForFrom` (как в этапе 8; у настроения оно другое, поэтому
  брать из ответа, не зашивать).
- Настройки передаются не здесь: в `session/new` **[НЕ ПРОВЕРЕНО, по памяти]**
  их нет, действуют сохранённые у станции (`settings2` из `…/info`).

### Настройки волны: только seeds новой сессии (сверка лида)

Старые `POST /rotor/station/{station}/settings2` и `settings3` Яндекс
отключил, **ими не пользуемся** (прежние догадки про `settings3` сняты).
Настройки передаются seeds в `POST /rotor/session/new` вместе с
`user:onyourwave`:

| Настройка | Seed |
|---|---|
| Настроение | `settingMoodEnergy:{fun, active, calm, sad}` |
| Характер | `settingDiversity:{favorite, popular, discover}` |
| Язык | `settingLanguage:{russian, not-russian, without-words}` |
| Занятие | seed станции `activity:{tag}` из `rotor/stations/dashboard` или `list` |

Значение «любое»/«по умолчанию» — seed не передаём.

**Занятие вместе с `user:onyourwave` или вместо?** Источники молчат (PR и
`RotorSettings` описывают только настройки при `user:onyourwave`). Реализовано
**вместо**: `["activity:{tag}", настройки…]`; если вживую сервер отвергнет или
проигнорирует настройки при занятии, пробовать `["user:onyourwave",
"activity:{tag}", …]`. Список занятий — `GET /rotor/stations/list?language=ru`
(станции с `station.id.type == "activity"`, подпись — `station.name`).
Выбор хранится в `config.toml` (`[wave_settings]`). Пример:
`{"seeds":["user:onyourwave","settingMoodEnergy:calm","settingDiversity:favorite"],...}`.
Смена настроек = **новая сессия** (очередь отбрасывается). Записи в аккаунт
нет. Источники (сверил лид): PR vornashev/omarchy-yandex-music#4, документация
yandex-music.readthedocs.io (`RotorSettings`). **Вживую формат seeds не
проверен** (см. «Что проверить вживую»).

### Что менять в нашем коде (для шага UI/кода, справка)

- `src/api/rotor.rs`: `start_wave(http, token)` с зашитым `WAVE_SEED` →
  `start_wave(http, token, seed)`; хук `PLASTINKA_TEST_ROTOR` расширить
  вариантом с настроением.
- Новый запрос `fetch_stations(http, token)` (dashboard) с мягким разбором
  `{id, name, icon, idForFrom, settings2, restrictions2}` на элемент.
- Меню по таблетке «Мне нравится ⌄» (`source_pill(label, None)` в
  `ui/library/wave.rs`): список из каталога (настроения, занятия) — что
  показывать, спросить пользователя (вопрос в handoff).
- Чтение `/stations/…` — это не запись в аккаунт, безопасно; запись —
  только лайки/дизлайки (настройки волны — seeds, записи нет).

## 5. souvlaki (медиаклавиши, SMTC, MPRIS)

**Всё прочитано в исходниках souvlaki 0.9.3 [souvlaki]**, если не помечено
иначе.

### Версия, лицензия, зависимости

- Версия **0.9.3** (последняя в реестре на 2026-11-10; `rust-version` 1.87,
  edition 2024), лицензия **MIT**, репозиторий `Sinono3/souvlaki`. Файл
  `LICENSE` в пакете: MIT, © 2021 Aldo Acevedo. GPL/LGPL нет — подключать
  можно, но **это новая зависимость → решение пользователя (CLAUDE.md,
  правило 3, handoff)**; в список сторонних лицензий добавить.
- Фичи: `default = ["use_dbus"]`, `use_dbus` (= `dbus` + `dbus-crossroads`,
  нужна системная `libdbus-1` и `pkg-config` при сборке), `use_zbus`
  (= `zbus` + `zbus_macros` + `zvariant` + `pollster`, чистый Rust). Они
  взаимоисключающие (`compile_error!`).
- **Windows**: `windows` 0.62 (фичи `Foundation`, `Media`,
  `Win32_Foundation`, `Win32_System_WinRT`, `Storage_Streams`). Версия
  `windows 0.62.2` **уже в нашем `Cargo.lock`** (через `cpal` и `mundy`),
  новая лишь добавкой фич; чужих тяжёлых зависимостей нет. (MIT OR
  Apache-2.0 у `windows` — по памяти, проверить `cargo metadata`.)
- **Linux** (`cfg(all(unix, not(macos/ios/android)))`): `thiserror` 2.0 +
  либо `dbus` 0.9.5 и `dbus-crossroads` 0.5.0, либо `zbus` ≥ 5.15,
  `zbus_macros`, `zvariant` ≥ 5.11, `pollster` 1. `zbus` 5.19.0 и
  `pollster` 0.4 уже в `Cargo.lock` (`zbus` через `mundy`/`iced` и
  keyring-хранилище; `pollster` через `rfd`). **Рекомендация: `use_zbus`**
  — без системной `libdbus` и `pkg-config` для самой souvlaki (`alsa-lib` и
  `dbus`/`pkgconf` из README остаются для других крейтов). Подключение
  по таблице целей:
  `[target.'cfg(unix)'.dependencies] souvlaki = { version = "0.9", default-features = false, features = ["use_zbus"] }`
  и отдельно под `cfg(windows)` с `default-features = false` (на Windows
  фичи dbus/zbus не используются и не тянутся). **Риск
  [НЕ ПРОВЕРЕНО]**: зависит от того, какими фичами `zbus` собирается у
  `mundy`/keyring (`async-io` против `tokio`); souvlaki крутит свой
  `pollster::block_on` в отдельном потоке — это работает с `async-io`
  (по умолчанию), но проверить на сборке под Linux (на Windows не собрать;
  пользователь проверит на CachyOS).
- macOS (`objc`, `core-graphics`, …) нам не нужен (цели Windows и Linux).

### API

```rust
use souvlaki::{MediaControls, MediaControlEvent, MediaMetadata, MediaPlayback,
               MediaPosition, PlatformConfig, SeekDirection};

let config = PlatformConfig {
    display_name: "Plastinka",          // Linux: имя плеера (identity)
    dbus_name: "plastinka",            // Linux: org.mpris.MediaPlayer2.plastinka
    hwnd: Some(hwnd_ptr),              // Windows: обязателен; Linux: None
};
let mut controls = MediaControls::new(config)?;      // Result<_, souvlaki::Error>
controls.attach(move |event: MediaControlEvent| { /* Fn + Send + 'static */ })?;
controls.set_metadata(MediaMetadata {
    title: Some("…"), artist: Some("…"), album: Some("…"),
    cover_url: Some("https://…"), duration: Some(Duration::from_millis(…)),
})?;
controls.set_playback(MediaPlayback::Playing { progress: Some(MediaPosition(pos)) })?;
// также Paused { progress }, Stopped
controls.set_volume(0.8)?;   // только MPRIS
controls.detach()?;          // вызывается и из Drop
```

- `PlatformConfig { display_name, dbus_name, hwnd: Option<*mut c_void> }`
  (структура с `*mut c_void` — не `Send`, создавать и сразу отдавать в
  `new`). `dbus_name` — по правилам имён D-Bus: латиница, цифры, `_`,
  без точек в начале; итоговое имя шины — `org.mpris.MediaPlayer2.plastinka`.
- События `MediaControlEvent` (`#[non_exhaustive]`, нужна ветка `_ =>`):
  `Play`, `Pause`, `Toggle`, `Next`, `Previous`, `Stop`, `Seek(SeekDirection)`,
  `SeekBy(SeekDirection, Duration)`, `SetPosition(MediaPosition)`,
  `SetVolume(f64)`, `SetShuffle(bool)` (только macOS), `OpenUri(String)`,
  `Raise`, `Quit`. Что приходит где:
  - Windows: `Play`, `Pause`, `Stop`, `Next`, `Previous`, `Seek(Forward/Backward)`
    (кнопки FastForward/Rewind) и `SetPosition` (бегунок в оверлее). `Toggle`
    отдельно не приходит: кнопка «Play/Pause» оверлея шлёт то `Play`, то
    `Pause` по состоянию, которое мы выставили `set_playback`.
  - Linux (MPRIS): `Play`, `Pause`, `Toggle` (метод `PlayPause`), `Stop`,
    `Next`, `Previous`, `Seek*`, `SetPosition`, `SetVolume`, `OpenUri`,
    `Raise`, `Quit`.
- `attach` на Windows включает кнопки и вешает обработчики; **обработчик
  вызывается из потока WinRT/D-Bus, не из потока iced**, поэтому в нём
  только отправка сообщения в канал (`tokio::sync::mpsc::UnboundedSender`
  /`std::sync::mpsc`), которое затем отдаёт `Subscription` окна. Правило из
  handoff: событие — обычное сообщение, без собственных перерисовок.
  Обработчик не должен паниковать и блокироваться; внутри souvlaki
  обработчик лежит за `Mutex` (`lock().unwrap()` — у них, не у нас).
- Метаданные: `set_metadata` кладёт **только переданные `Some`-поля**; `None`
  не очищает прежнее значение на Windows (в `windows/mod.rs` `if let Some(…)`).
  При смене трека всегда передавать все поля (пустое — `Some("")`), иначе
  в оверлее останутся данные прошлого трека. `duration` задаёт и шкалу
  позиции (`UpdateTimelineProperties`). На Windows вызов `set_metadata` с
  `cover_url` из `file://` блокирует поток (`loader.join()`), с `https://`
  отдаёт загрузку системе.
- `set_playback`: на Windows `progress` обновляет позицию на шкале оверлея;
  вызывать при смене состояния и при перемотке (не на каждом кадре; раз в
  секунду достаточно, лучше только при изменениях: оверлей сам ведёт
  время между обновлениями — **НЕ ПРОВЕРЕНО**). На Linux позицию отдаёт
  свойство `Position` по запросу.
- Обложка (`cover_url`):
  - Windows: любая строка, годная для `Uri::CreateUri` (https работает;
    souvlaki отдаёт её системе), либо `file://путь` — тогда читается
    локальный файл (`StorageFile::GetFileFromPathAsync`) после отрезания
    `file://`: для `C:\…` нужно `file://C:\…` (не `file:///C:/…`).
    Берём **https-адрес обложки Яндекса** (`avatars.yandex.net/…/400x400`,
    уже строится у нас для плеера и Discord RPC): ничего на диск не пишем
    (правило 1: обложки на диск можно, но здесь не нужно).
  - Linux (MPRIS `mpris:artUrl`): строка как есть; по спецификации локальные
    файлы — `file://`, оболочки (GNOME, KDE) сами тянут https; точное
    поведение — **не проверено**.
- MPRIS: `mpris:trackid` у souvlaki всегда `"/"` (по спецификации должен быть
  уникальным путём) — частый `Seek`/`SetPosition` может не сработать у части
  клиентов; ограничение крейта, не меняем.
- Ошибки: все методы возвращают `Result<(), souvlaki::Error>`; наш код
  обрабатывает без `unwrap` — при ошибке просто отключаем системный плеер и
  пишем причину в строку состояния (не ронять окно). Внутри souvlaki на
  Linux (`zbus`-вариант) поток службы при сбое D-Bus делает
  `.unwrap()`: паника **в их потоке** (в stderr), а потом `set_*` вернёт
  `Error::ThreadPanicked`; программа не падает. Ограничить: не вызывать
  `attach` повторно, ошибки `set_*` — тихий выход.
- `Drop` вызывает `detach()`: при закрытии окна и выходе из аккаунта
  дропать `MediaControls`, иначе в Windows оверлей остаётся до выхода.
  `detach` на Linux ждёт завершения потока службы (`thread.join()`):
  вызывать не из async-потока, где это может заблокировать UI; при
  закрытии приложения — приемлемо.

### HWND окна iced/winit (Windows)

Уже решено в проекте: `src/ui/mod.rs:598–610` — `window::run(id,
native_handle)` возвращает `Message::NativeHandle(Option<isize>)`, а
`native_handle` (`src/ui/mod.rs:1598`) берёт `RawWindowHandle::Win32(h) →
h.hwnd.get()` (`window::raw_window_handle` из iced). Этим же способом
получаем HWND для souvlaki:

```rust
let ptr = hwnd as *mut std::ffi::c_void; // isize → указатель
```

(`with_exposed_provenance_mut` без `as`-преобразования с потерей, как в
`drag_pulse.rs`; тот модуль — единственное место, где разрешён `unsafe`.
Сам вызов `MediaControls::new` безопасный, `unsafe` у нас не нужен.)
Порядок: окно создано → пришло `NativeHandle(Some(hwnd))` → создаём
`MediaControls` (только если уже подтверждена подписка — правило 2: плееру
без Плюса не быть, значит и системный плеер не создаём до успешной
проверки), потом `attach`, потом `set_metadata`/`set_playback`. На Linux
`hwnd: None`, создаём там же без ожидания.

**Оговорки [НЕ ПРОВЕРЕНО]:**

- Нужен ли у приложения без пакета (не MSIX) особый AppUserModelID для
  отображения имени в оверлее Windows 11: по памяти без него оверлей
  показывает имя исполняемого файла, не «Plastinka». Проверить вживую, при
  необходимости — `SetCurrentProcessExplicitAppUserModelID` (потребует
  `windows-sys` фичу и, возможно, `unsafe`; отдельное решение).
- `MediaControls` на Windows: типы WinRT в `windows` 0.62 помечены
  `Send + Sync` для «агильных» объектов, а `SystemMediaTransportControls`
  таковым является — но **проверить компилятором**, можно ли держать его в
  состоянии окна и использовать из потока iced.
- Нет доказательства, что SMTC получает клавиши, пока окно без рамки/не в
  фокусе: SMTC привязывается к HWND и работает по глобальным медиаклавишам
  — проверять вживую (медиаклавиши пользователь нажимает сам; ввод в окно
  агентом не нужен).

### Наши правила для интеграции

- Подписка (правило 2): системный плеер создаётся после проверки Плюса и
  уничтожается при выходе из аккаунта/потере подписки; события, пришедшие
  без плеера, игнорируются.
- События ставятся в очередь сообщений окна; «Следующий» в волне — тот же
  `skip`, что у кнопки (handoff).
- Только метаданные (название, исполнители, альбом, https-обложка,
  длительность): в SMTC/MPRIS звук не передаётся, на диск ничего не пишется.
- Токен в метаданные и ошибки не попадает.

## 6. История прослушиваний

Решение пользователя (2026-11-12): показываем серверную историю, и Plastinka
сообщает Яндексу о прослушиваниях (`play-audio`), как официальный клиент.
Это согласованная запись в аккаунт, **только она**. Весь раздел **вживую не
проверен**; формат истории пересказан лидом по справке MarshalX (код не
брался).

### `POST /play-audio`

**[крейт]** `src/api/track/play_audio.rs` (`PlayAudioOptions`): поля
`track_id`, `album_id`, `playlist_id`, `play_id`, `from`, `from_cache`, `uid`,
`track_length_seconds`, `total_played_seconds`, `end_position_seconds`,
`timestamp`, `client_now`. **Но тело запроса в крейте не реализовано**
(`options()` — `todo`, пустое), поэтому форма собрана **[по памяти, справка
MarshalX]**: `application/x-www-form-urlencoded`, имена с дефисами:

| Поле формы | Значение |
|---|---|
| `track-id` | id трека (для замены FckCensor — id реально игравшего) |
| `album-id` | id альбома (если известен и это тот же трек) |
| `from` | `plastinka` (наш клиент, без личных данных; допустимые значения у сервера не известны) |
| `from-cache` | `false` |
| `play-id` | случайный UUID v4 на каждый запуск трека (крейт `uuid` уже в проекте) |
| `uid` | id аккаунта |
| `timestamp` | когда трек начался, RFC 3339 UTC с миллисекундами |
| `client-now` | когда отправлено, тот же формат |
| `track-length-seconds` | длительность трека, целые секунды (если известна) |
| `total-played-seconds`, `end-position-seconds` | сколько сыграно, целые секунды (у нас равны: перемотку отдельно не учитываем) |

Заголовки — как у остальных запросов (`Authorization`, `X-Yandex-Music-Client`).
Ответ — `{"result":"ok"}`, нам не нужен (проверяем только код).

Когда шлём (`src/api/play_audio.rs`, `Tracker`): **дважды за трек** — когда
зазвучал (сыграно 0, один раз; перемотка и буферизация второго не дают) и когда
сменился, кончился или остановился (сколько сыграно; доигранный до конца —
вся длительность). Трек, который не зазвучал, не шлём вовсе. Не шлём о своём
локальном файле FckCensor и о поиске версии без цензуры (путь `Search`).
Волна тоже шлёт `play-audio` (кроме rotor-feedback), как официальный клиент —
**проверить вживую**, нужно ли это сервису. Только при подтверждённом Плюсе
(библиотека создаётся только с пропуском). Ошибки: одна строка в stderr без
токена, игре не мешают. С тестовым токеном в сеть не уходит (строка
`play-audio (тест) …` в stderr).

### `GET /music-history?fullModelsCount=N`

Ответ **[по справке MarshalX, не проверено]**:

```json
{"result":{"historyTabs":[
  {"date":"2026-11-12","items":[
    {"context":{"type":"album","data":{"itemId":{"id":"10101"},
                "fullModel":{"id":10101,"title":"…"}}},
     "tracks":[{"type":"track","data":{"itemId":{"trackId":"1","albumId":"2"},
                "fullModel":{…полный Track…}}}]}]}]}}
```

- `historyTabs[]` → `{date, items[]}`; группа → `{context, tracks[]}`.
- `context.data.itemId`: альбом и исполнитель `{id}`, плейлист `{uid, kind}`,
  волна `{seeds}`; `fullModel` — `{album|artist|playlist|wave, …}` (у нас
  берутся `title`, `name`, `owner.uid`, `kind`). Типы: `album`, `artist`,
  `playlist`, `wave`; незнакомый тип — подпись «Подборка».
- Полная модель трека (`fullModel`) приходит только для первых `N` треков
  (просим 50); у остальных только `itemId.trackId`. Такие треки догружаем
  **одним набором по id нашим батчем `POST /tracks`** (уже есть, `fetch_tracks`);
  `POST /music-history/items` не используем (выбрал одно).
- Дата — `YYYY-MM-DD` (допускается хвост со временем). Подписи: «Сегодня»,
  «Вчера», «12 ноября» (с годом, если не нынешний). «Сегодня» считается по UTC.
- Разбор мягкий: непонятный день, группа или трек пропускается; без
  `historyTabs` — «неожиданный ответ». Берём не больше 30 дней и 300 треков;
  в окне показывается не больше 200 строк.
- Тестовый хук `PLASTINKA_TEST_HISTORY` (`sample`, `empty`, `error`, `slow`).

## Расхождения и что проверить вживую

Расхождения между источниками и нашим кодом:

1. **Подпись текстов vs `get-file-info`**: разные порядок полей и обработка
   `=` (см. раздел 1). У нас в `stream.rs` функция только под файл-инфо;
   для текстов нужна отдельная. Согласовано с крейтом, живой ответ не
   сверен.
2. **Модель текстов в крейте строгая** (все пять полей и `lyric_id: u32`);
   у нас разбор мягкий, нужен только `downloadUrl`.
3. **`rotor/station/{id}/tracks` в крейте — с ошибкой сборки адреса**
   (`&queue` без `?`), известно с этапа 8; к сессионному пути, который мы
   используем, отношения не имеет.
4. **Смена настроек**: в крейте метода нет; `settings3` отключён, работаем
   seeds новой сессии (лид сверил по PR и документации, вживую не проверено).
5. **souvlaki на Linux**: два бэкенда с разными свойствами (dbus —
   «стабильнее, нативная `libdbus`», zbus — чистый Rust; по README souvlaki
   `dbus` может быть нестабильным «consistency»). Мы выбираем `use_zbus`
   из-за отсутствия системной зависимости; решение зависит от проверки на
   CachyOS.

Проверить вживую (токен у пользователя; агент токен не использует):

- `GET /tracks/{id}/lyrics`: достаточно ли подписи «id + ts», нужен ли
  `X-Yandex-Music-Client` особого вида (в крейте для текстов своего нет),
  ответ для трека без текста (код и тело), `downloadUrl` — хост,
  требуется ли авторизация, срок жизни, формат LRC (построчный/посословный),
  разметка `[offset]`, вид `TEXT`;
- доля треков с `lyricsInfo` и совпадают ли флаги с реальностью;
- LRCLib: ответ `/api/get` для трека с разными вариантами названия
  (с «(feat. …)», «Remastered»), для русских треков, 404 и тело; нужен ли
  `album_name`; поведение `/api/search`; допуск по `duration`;
- rotor: `dashboard` и `list` — реальные `type`/`tag` настроений и занятий,
  `restrictions2.possibleValues`, `idForFrom` у не-«Моей волны» станций,
  принимает ли `session/new` настроения в `seeds` и совпадает ли формат
  `from` в событиях; формат seeds `settingMoodEnergy:`/`settingDiversity:`/`settingLanguage:`/
  `activity:{tag}` в `session/new` вживую не проверен;
- история: формат `music-history` и поля формы `play-audio` (имена, `from`, нужен ли `play-audio` у волны), принимает ли сервер `from=plastinka`;
- souvlaki: SMTC при окне iced на Windows (оверлей показывает название и
  обложку, кнопки доходят, имя приложения), MPRIS на CachyOS (GNOME/KDE,
  `playerctl`), сборка `use_zbus` вместе с `zbus` из `mundy`/keyring.

## Открытые вопросы

Все пять вопросов первой редакции закрыты разделом «Решения».
