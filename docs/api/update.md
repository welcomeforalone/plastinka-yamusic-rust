# Автообновление десктопа: спецификация запросов

Источник — релизы GitHub-репозитория `UPDATE_REPO` (`src/update.rs`). Все запросы
без токена (ни Яндекса, ни GitHub). Код других проектов не переносился; факты об
API GitHub — из официальной документации REST API.

## `GET https://api.github.com/repos/{UPDATE_REPO}/releases/latest`

- Заголовки: `Accept: application/vnd.github+json`,
  `X-GitHub-Api-Version: 2022-11-28`, `User-Agent: Plastinka/<версия>`.
- Нужные поля ответа (остальные игнорируются):

```json
{
  "tag_name": "v0.0.6",
  "html_url": "https://github.com/<repo>/releases/tag/v0.0.6",
  "name": "Версия 0.0.6",
  "draft": false,
  "prerelease": false,
  "assets": [
    {"name": "Plastinka.exe", "size": 36000000,
     "browser_download_url": "https://github.com/<repo>/releases/download/v0.0.6/Plastinka.exe"}
  ]
}
```

- `tag_name` — `v0.0.6` или `0.0.6`; сравнивается с `CARGO_PKG_VERSION`
  по числовым частям (`0.0.10 > 0.0.9`). Суффикс `-…` (пререлиз) и неразобранная
  версия — «обновления нет». `draft` и `prerelease` игнорируются
  (`releases/latest` их и так не отдаёт).
- Ошибки: `404` — релиза нет или репозиторий закрыт (без авторизации приватный
  репозиторий отвечает 404): «Релиз не найден…»; `403`/`429` — лимит GitHub
  (60 запросов в час на адрес без токена): «GitHub временно ограничил число
  проверок»; прочие коды — «GitHub ответил ошибкой N»; не JSON или нет
  `tag_name` — «Не удалось разобрать ответ GitHub». Сетевые ошибки — `NetError`.
- Размер ответа ограничен 1 МБ.

## Файлы релиза

| Платформа | Файл | Контрольная сумма |
|---|---|---|
| Windows | `Plastinka.exe` | `Plastinka.exe.sha256` |
| Linux x86_64 | `plastinka-linux-x86_64.tar.gz` (внутри `plastinka-linux-x86_64/Plastinka`) | `<файл>.sha256` |
| macOS | не качаем, только ссылка `html_url` | — |

`.sha256` — формат `sha256sum`: `<64 hex>  <имя>`. Берётся первое слово, нужны
ровно 64 hex-символа; сравнение без учёта регистра. Нет файла или его `.sha256`
в релизе — только уведомление со ссылкой.

## Скачивание

- `GET browser_download_url` (редирект на `*.githubusercontent.com`). Редиректы:
  только `https` и хосты `github.com`, `api.github.com`, `*.githubusercontent.com`,
  не больше 5; иное — ошибка. `browser_download_url` и `html_url` из ответа тоже
  проверяются на хост; чужой `html_url` заменяется ссылкой на `/releases/latest`.
- `connect_timeout` 10 с, `read_timeout` 30 с, общего таймаута нет. Размер файла
  ограничен 300 МБ, `.sha256` — 4 КБ.
- Файл держится в памяти; SHA-256 проверяется до записи на диск. Не совпало —
  «Файл обновления повреждён (контрольная сумма не совпала). Обновление не
  установлено», на диск ничего не пишется.

## Замена файла

- Windows: `Plastinka.exe.new` (запись, `sync_all`) → запущенный exe
  переименовывается в `Plastinka.exe.old` → `.new` на место. Ошибка — откат.
  `.old` удаляется при следующем запуске.
- Linux (только бинарник из `${XDG_BIN_HOME:-$HOME/.local/bin}/Plastinka`):
  `tar -xzOf - plastinka-linux-x86_64/Plastinka` (архив в stdin, файл из stdout,
  без временных файлов), запись `Plastinka.new` с правами 755, `rename` поверх.
- macOS и прочие установки: только уведомление.
- Перезапуск: новая копия с `--after-update` (скрытый флаг, пока ничего не
  делает).

## Отладочные хуки (только `debug_assertions`)

`PLASTINKA_TEST_UPDATE_URL` (адрес вместо API; разрешены http и любые хосты),
`PLASTINKA_TEST_UPDATE_VERSION` (версия этой сборки),
`PLASTINKA_TEST_UPDATE_TARGET` (что заменять; без неё при хуке замены нет).
Сценарии — в README, «Тестовые сценарии».

## Что проверить вживую

- Ответ настоящего `releases/latest`, когда репозиторий станет публичным
  (имена файлов в `release.yml`, формат `.sha256`).
- Редирект скачивания на `*.githubusercontent.com` (включая `release-assets`).
