# Plastinka

Неофициальный десктопный клиент Яндекс Музыки на Rust для Windows и Linux.
Быстрая нативная замена официальному клиенту на Electron.

> Неофициальный клиент, не связан с ООО «Яндекс».

- Только потоковое воспроизведение: треки не сохраняются на диск, кнопки
  «скачать» нет. Кэшируются только метаданные и обложки.
- Для воспроизведения нужна активная подписка Плюс.
- Токен хранится в системном хранилище секретов (Windows Credential Manager,
  Secret Service в Linux).

## Статус

| Этап | Что | Готово |
|---|---|---|
| 1 | Структура проекта | ✅ |
| 2 | Вход по коду устройства | |
| 3 | Проверка подписки | |
| 4 | «Мне нравится» в консоли | |
| 5 | Воспроизведение трека потоком | |
| 6 | Окно: список треков и плеер | |
| 7 | Поиск, плейлисты, альбомы, исполнители | |
| 8 | «Моя волна» | |
| 9 | Медиаклавиши, системный плеер, тексты | |

## Требования

**Windows:** Rust stable через [rustup](https://rustup.rs) и Visual Studio
(или Build Tools) с компонентом «Разработка классических приложений на C++».

**Linux (Arch, CachyOS):**

```sh
sudo pacman -S --needed base-devel alsa-lib dbus pkgconf
```

**Linux (Debian, Ubuntu):**

```sh
sudo apt install build-essential pkg-config libasound2-dev libdbus-1-dev
```

ALSA нужна для звука, D-Bus — для хранилища секретов и системного плеера.

## Запуск

```sh
cargo run -- --help    # список команд
cargo run -- about     # о программе и где лежат данные
```

Сборка релиза: `cargo build --release`. Бинарник: `target\release\plastinka.exe`
в Windows, `target/release/plastinka` в Linux.

## Проверки

```sh
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo test
```

В рабочем коде линты в `Cargo.toml` запрещают всё, что может паниковать:
`unwrap`, `expect`, `panic!`, `unreachable!`, `println!`/`eprintln!` (вместо них
`writeln!`), индексацию и срезы (вместо них `get`). В тестах это разрешено
(`clippy.toml`), но только внутри `#[test]` и `#[cfg(test)]`. Поэтому файлы
в `tests/` начинаются с `#![cfg(test)]`.

## Где лежат данные

| | Windows | Linux |
|---|---|---|
| Настройки | `%APPDATA%\Plastinka\config` | `~/.config/plastinka` |
| Кэш метаданных и обложек | `%LOCALAPPDATA%\Plastinka\cache` | `~/.cache/plastinka` |

Каталоги создаются при первой записи. Точные пути показывает `cargo run -- about`.

## Структура

| Модуль | За что отвечает |
|---|---|
| `src/main.rs` | Командная строка (до появления окна на этапе 6) |
| `src/api` | Запросы к API музыки через крейт `yandex-music` |
| `src/auth` | Вход по коду устройства и выход |
| `src/player` | Потоковое воспроизведение (symphonia, rodio) |
| `src/storage` | Каталоги настроек и кэша, токен в keyring |
| `src/ui` | Окно на iced |

## Тестовые сценарии

Как воспроизвести ошибки, не трогая настройки системы:

| Сценарий | Как |
|---|---|
| Нет сети | `HTTPS_PROXY=http://127.0.0.1:9` в окружении процесса (с этапа 2, когда появятся запросы) |
