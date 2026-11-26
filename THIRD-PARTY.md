# Сторонние компоненты и источники данных

Plastinka распространяется под [GPL-3.0-or-later](LICENSE). Ниже — что из
чужого входит в программу или используется во время работы.

## Входит в программу

| Что | Где | Лицензия | Источник |
|---|---|---|---|
| Шрифт Inter (урезан до нужных символов) | `assets/fonts` | SIL OFL 1.1, `assets/fonts/LICENSE-Inter.txt` | https://github.com/rsms/inter |
| Иконки Lucide и свои в том же стиле | `assets/icons` | ISC, `assets/icons/LICENSE-lucide.txt` | https://github.com/lucide-icons/lucide |
| Symphonia (декодеры MP3, AAC, FLAC, Vorbis, PCM) | крейты `symphonia-*` | MPL-2.0, код не изменён | https://github.com/pdeljanov/Symphonia |
| Остальные крейты Rust | `Cargo.lock` | MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0 и подобные | crates.io |
| Библиотеки Android (AndroidX, Compose, Media3, Kotlin) | `android/gradle/libs.versions.toml` | Apache-2.0 | https://developer.android.com |

GPL- и LGPL-кода в сборке нет. Несколько крейтов выпущены под выбором из
нескольких лицензий, где одна из них GPL или LGPL (`self_cell`, `r-efi`). Для
них используется MIT или Apache-2.0.

Полный список крейтов с лицензиями и ссылками на исходники:

```sh
cargo metadata --format-version 1
```

или `cargo about` / `cargo license`, если они установлены.

## Используется во время работы

Эти сервисы не входят в программу: Plastinka обращается к ним по сети и
кэширует на диске только метаданные.

| Что | Зачем | Когда |
|---|---|---|
| API Яндекс Музыки и Яндекс ID | вход, каталог, поток, тексты | всегда |
| [Hazzz895/FckCensorData](https://github.com/Hazzz895/FckCensorData) | названия и обложки без цензуры | только с включённым плагином FckCensor |
| [slopless.art](https://slopless.art) | метки ИИ-исполнителей | только с включённым плагином Slopless |
| [LRCLib](https://lrclib.net) | синхронный текст, если у Яндекса его нет | при открытии текста песни |
| GitHub Releases этого репозитория | проверка и загрузка обновлений | только по кнопке или с включённым автообновлением |

## Идеи, а не код

Плагины FckCensor, Slopless, CopyTrackName, VibeColorizer и YandexMusicTime
повторяют идеи аддонов для PulseSync и Яндекс Музыки. Они написаны на Rust
заново, код аддонов не использовался. Официальный клиент, Cassette,
MarshalX/yandex-music-api и другие проекты служили только справкой о
поведении API.

Plastinka — неофициальный клиент, не связан с ООО «Яндекс». «Яндекс» и
«Яндекс Музыка» — товарные знаки их владельцев.
