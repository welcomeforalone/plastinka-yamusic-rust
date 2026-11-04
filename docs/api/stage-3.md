# Этап 3: проверка подписки — спецификация API

Дополняет раздел про `account/status` в `docs/api/stage-2.md`.

## Итог

1. Подписку смотрим в том же ответе `GET /account/status`, что проверяет
   токен. Отдельного запроса не нужно.
2. Признак подписки — `result.plus.hasPlus == true`. Только это значение
   (булево `true`) даёт пропуск к плееру. Отсутствие поля, `null`, другой тип —
   «подписка не подтверждена».
3. `result.account.serviceAvailable == false` — сервис недоступен в регионе.
   Пропуска нет, даже если `hasPlus` равно `true`. Нет поля — считаем, что
   доступен: в анонимном ответе оно есть, в моделях крейта и MarshalX
   обязательное. `null` и не-bool — «подписка не подтверждена».
4. Сроки и вид подписки (`result.subscription`) — только для показа
   пользователю. На пропуск не влияют, разбираются мягко: непонятный блок
   просто не показывается.
5. Любая ошибка запроса (сеть, 401/403, 5xx, 429, битый JSON, нет `uid`) —
   это не «подписки нет», а «проверить не удалось». Пропуска нет.

## `GET https://api.music.yandex.net/account/status`

- Заголовки: `Authorization: OAuth <token>`,
  `X-Yandex-Music-Client: YandexMusicAndroid/24023621`. Параметров нет.
- Ответ 200: `{"invocationInfo":{…},"result":{…}}`.

### Нужные поля `result`

| Поле | Тип | Зачем |
|---|---|---|
| `account.uid` | число | Токен рабочий (этап 2) |
| `account.login`, `account.displayName` | строка | Имя для вывода |
| `account.serviceAvailable` | bool | Доступен ли сервис в регионе |
| `account.region`, `account.regionCode` | число, строка | Для сообщения о регионе (`regionCode` — двухбуквенный, например `am`) |
| `account.child` | bool | Детский аккаунт. На пропуск не влияет, только справка |
| `account.nonOwnerFamilyMember` | bool | Участник чужой семейной подписки |
| `plus.hasPlus` | bool | **Есть ли Плюс** |
| `plus.isTutorialCompleted`, `plus.migrated` | bool | Не нужны |
| `subscription` | объект | Сроки и вид подписки (ниже) |
| `permissions.values` | массив строк | Права: `landing-play`, `feed-play`, `radio-play`, `mix-play`, `radio-skips`, `play-radio-full-tracks`. Не используем (см. расхождения) |
| `advertisement` | строка | Рекламный текст. Не показываем |

### `result.subscription`

| Поле | Тип | Что |
|---|---|---|
| `autoRenewable` | массив | Подписки с автопродлением |
| `familyAutoRenewable` | массив | Семейные подписки с автопродлением |
| `nonAutoRenewable` | объект `{start, end}` | Подписка без автопродления |
| `nonAutoRenewableRemainder` | объект | Остаток подписки без продления. Не используем |
| `operator` | массив | Подписки через оператора связи. Не используем |
| `end` | строка | Дата окончания |
| `hadAnySubscription` | bool | Была ли подписка раньше |
| `canStartTrial` | bool | Можно ли начать пробный период |
| `mcdonalds` | bool | Не описано. Не используем |

Элемент `autoRenewable` / `familyAutoRenewable`: `expires` (дата),
`vendor`, `vendorHelpUrl`, `product` (объект), `productId`, `orderId`,
`finished` (bool, автопродление закончилось), `masterInfo` (владелец
семейной подписки).

Как показываем:

- **срок** — самая поздняя дата из `autoRenewable[].expires`,
  `familyAutoRenewable[].expires`, `nonAutoRenewable.end`, `end`. Даты в
  ISO 8601 (`2027-05-12T00:00:00+03:00`), показываем день в виде
  `12.05.2027`. Непонятная дата пропускается;
- **автопродление** — есть элемент `autoRenewable` или `familyAutoRenewable`
  с `finished == false`;
- **семейная** — непустой `familyAutoRenewable` или
  `account.nonOwnerFamilyMember == true`;
- **пробный период** — при отсутствии Плюса и `canStartTrial == true`
  подсказываем, что его можно оформить. Признака «сейчас идёт пробный
  период» в источниках нет (`pretrialActive` не описан).

### Примеры

Без токена (проверено вживую 2026-11-04): `plus` и `subscription` нет.

```json
{"result":{"account":{"now":"2026-11-04T16:33:44+03:00","region":168,
 "regionCode":"am","serviceAvailable":true},
 "permissions":{"until":"…","values":["landing-play","feed-play","mix-play"],
 "default":["landing-play","feed-play","mix-play"]},
 "advertisement":"Оформите постоянную подписку – первый месяц бесплатно!"},
 "invocationInfo":{…}}
```

С подпиской (по моделям источников, вживую не видели):

```json
{"result":{"account":{"uid":42,"login":"user","serviceAvailable":true,"child":false},
 "plus":{"hasPlus":true,"isTutorialCompleted":true},
 "subscription":{"autoRenewable":[{"expires":"2027-05-12T00:00:00+03:00",
   "vendor":"Yandex","finished":false}],
  "familyAutoRenewable":[],"hadAnySubscription":true,"canStartTrial":false}}}
```

### Ошибки и крайние случаи

| Ответ | Результат |
|---|---|
| 200, есть `uid`, `hasPlus: true`, `serviceAvailable` не `false` | Подписка есть, пропуск выдан |
| 200, есть `uid`, `hasPlus: false` | «Нужна подписка» |
| 200, есть `uid`, нет `plus` или `hasPlus` не bool | «Подписка не подтверждена», пропуска нет |
| 200, `serviceAvailable: false` | «Сервис недоступен в регионе», пропуска нет |
| 200 без `uid` (анонимный) | Токен не рабочий (этап 2) |
| 401, 403 | Токен не действует, повторный вход |
| 5xx, 429 | Временный сбой, повторить позже |
| Прочий код | Неожиданный ответ |
| Не JSON, нет `result.account` | Непонятный ответ |
| Сеть | Ошибка сети, не «подписки нет» |

Блок `subscription` с неожиданными типами полей не ломает проверку:
`plus.hasPlus` и `account` разбираются отдельно.

## Крейт `yandex-music` 0.7.0

- `AccountStatus` и `AccountPlus`: поля `child`, `serviceAvailable`, `plus`,
  `permissions`, `isTutorialCompleted` обязательные. Блока `subscription`
  нет вовсе. На анонимном ответе разбор падает, 401 превращается в
  строковую ошибку `RequestFailed`.
- Поэтому метод крейта не используем, разбор свой (`src/api/account.rs`).

## Источники

1. Модели крейта `yandex-music` 0.7.0: `src/model/account/status.rs`,
   `src/model/account/mod.rs`.
2. MarshalX/yandex-music-api, модели `Status`, `Plus`, `Subscription`,
   `AutoRenewable`, `NonAutoRenewable`, `Permissions` (только как справка о
   полях).
3. acherkashin/yandex-music-open-api: схемы `Status`, `Account`,
   `Permissions`, `Subscription`.
4. Живой запрос без токена и с фальшивым токеном (2026-11-04): анонимный
   ответ выше, фальшивый токен — 401.

## Расхождения

- OpenAPI считает `subscription` и `pretrialActive` обязательными, крейт
  вообще не знает `subscription`, MarshalX делает всё необязательным.
  Анонимный ответ показывает, что `plus` и `subscription` бывают не всегда.
  Вывод: все поля, кроме `uid`, необязательные.
- В OpenAPI у `Subscription` только `hadAnySubscription`, `canStartTrial`,
  `mcdonalds`. Списки подписок и даты описаны только у MarshalX.
- `permissions.values` в анонимном ответе содержит `*-play`. По названиям
  похоже на право слушать подборки, но связь с полными треками не описана.
  Пропуск строим только на `hasPlus`, как в известных клиентах.

## Проверить вживую

Агент вживую видел только анонимный ответ. Пользователю при приёмке:

- `plastinka account` с подпиской: строка о Плюсе, срок и автопродление
  совпадают с id.yandex.ru;
- если есть аккаунт без Плюса — сообщение «Нужна подписка»;
- семейная подписка (если есть) — помечена как семейная.
