# Выпуск релиза

Один тег `vX.Y.Z` — один релиз сразу для Windows, Linux, macOS и Android.

## Что выходит в релизе

| Файл | Платформа |
|---|---|
| `Plastinka.exe` | Windows, без архива |
| `plastinka-linux-x86_64.tar.gz` | Linux x86_64 |
| `plastinka-macos.zip` | macOS (Apple Silicon и Intel), внутри `Plastinka.app` |
| `plastinka-android.apk` | Android |
| `<файл>.sha256` у каждого из четырёх | контрольная сумма файла |
| `SHA256SUMS.txt` | контрольные суммы всех четырёх файлов |

Имена файлов неизменны: по ним ищет встроенное автообновление. Менять их
можно только вместе с кодом автообновления.

## Подготовка версии

Версия одна на все платформы:

- `version` в `Cargo.toml` и тот же номер в `Cargo.lock` (после правки
  `cargo update -p plastinka --offline` или любая сборка; CI собирает с
  `--locked`);
- `versionName` (тот же номер) и `versionCode` (+1 на каждый релиз) в
  `android/app/build.gradle.kts`.

`release.yml` сверяет эти значения с тегом и останавливается при расхождении.

## Выпуск

1. Закоммитить изменения в `main`. В `main` пушат и другие участники: перед
   тегом `git fetch` и сверить `git log main..origin/main`.
2. Поставить тег: `git tag -a vX.Y.Z -m "Plastinka X.Y.Z"`.
3. Отправить ветку и тег: `git push origin main` и `git push origin vX.Y.Z`.
4. Workflow `release.yml` проверит версии, соберёт четыре платформы и создаст
   черновик релиза «Plastinka X.Y.Z».
5. Проверить черновик: все файлы на месте, `SHA256SUMS.txt` есть, отпечаток
   APK виден в логе job `android` (шаг «Verify signature»).
6. Заполнить раздел «Что нового» и опубликовать релиз вручную.
7. После публикации workflow `pages.yml` сам пересоберёт сайт: версия на
   нём — последний опубликованный релиз.

## Сайт

Страницы лежат в `site/` (`index.html` — русская, `en/index.html` —
английская), скриншоты берутся из `docs/screenshots`. Сайт публикуется на
GitHub Pages workflow `pages.yml` при изменении этих файлов в `main`, после
выхода релиза и вручную (`gh workflow run pages.yml`). Собрать локально:
`sh site/build.sh _site http://localhost:8000 1.0.0`, затем открыть `_site`
любым статическим сервером. Счётчиков и аналитики на сайте нет.

## Повторная сборка

`gh workflow run release.yml -f tag=vX.Y.Z` — черновик обновится (файлы
заменятся, заметки останутся). Опубликованный релиз workflow не трогает и
завершается ошибкой.

## Сборки по push в main

Каждый push в `main` собирает платформы по отдельным workflow
(`build-*.yml`). Результат — артефакты на 30 дней, в релиз они не попадают.

## Подпись Android

Ключ подписи хранит владелец проекта, в репозитории его нет. Резервная копия
обязательна: без этого ключа обновления APK не встанут поверх установленного.

Секреты GitHub Actions:

- `ANDROID_KEYSTORE_BASE64` — keystore в base64;
- `ANDROID_KEYSTORE_PASSWORD` — пароль хранилища;
- `ANDROID_KEY_ALIAS` — имя ключа;
- `ANDROID_KEY_PASSWORD` — пароль ключа.

Как задать:

```sh
base64 -w0 plastinka-release.jks | gh secret set ANDROID_KEYSTORE_BASE64
gh secret set ANDROID_KEYSTORE_PASSWORD
gh secret set ANDROID_KEY_ALIAS
gh secret set ANDROID_KEY_PASSWORD
```

Для паролей `gh secret set ИМЯ` без `--body` спросит значение из stdin, так оно
не попадёт в историю командной строки.

Отпечаток SHA-256 сертификата подписи:

```
ef5e8c87698a9fe5533dcda15789227ce6e42acd6c55e1f9c65d3caad1c576fa
EF:5E:8C:87:69:8A:9F:E5:53:3D:CD:A1:57:89:22:7C:E6:E4:2A:CD:6C:55:E1:F9:C6:5D:3C:AA:D1:C5:76:FA
```

Проверить у скачанного APK (`apksigner` входит в Android SDK build-tools):

```sh
apksigner verify --print-certs plastinka-android.apk
```

Значение в строке `certificate SHA-256 digest` (в зависимости от версии
`apksigner` она начинается с `Signer #1` или `V2 Signer:`) должно совпасть с
отпечатком выше.
Тот же отпечаток workflow `build-android.yml` сверяет при каждой сборке.

При смене ключа поменять отпечаток в `build-android.yml` (переменная
`ANDROID_CERT_SHA256`) и в этом файле. Пользователям придётся переустановить
приложение: обновление поверх APK с другой подписью Android не примет.

## Локальная release-сборка APK

Подпись задаётся файлом `android/keystore.properties` (по образцу
`android/keystore.properties.example`) или переменными окружения
`ANDROID_KEYSTORE_PATH`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`,
`ANDROID_KEY_PASSWORD`. Без них release-сборка получится неподписанной
(`app-release-unsigned.apk`). Debug-сборке ключ не нужен.

## macOS

Бандл `Plastinka.app` подписан ad-hoc, без нотаризации, поэтому при первом
запуске Gatekeeper предупредит о неизвестном разработчике. Открыть можно так:
«Системные настройки → Конфиденциальность и безопасность → Всё равно
открыть», либо в терминале:

```sh
xattr -dr com.apple.quarantine Plastinka.app
```
