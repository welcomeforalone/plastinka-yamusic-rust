# Устройство проекта

## Структура

| Модуль | За что отвечает |
|---|---|
| `src/main.rs` | Запуск окна и командная строка для отладки |
| `src/texts.rs` | Склонения, длительности и время в тексте интерфейса |
| `src/net.rs` | Общий HTTP-клиент с таймаутами, сетевые ошибки |
| `src/api` | Запросы к API музыки: статус аккаунта и подписки, пропуск к плееру, «Мне нравится» и треки, каталог (`catalog`: поиск, альбом, исполнитель, плейлист, коллекция) |
| `src/auth` | Вход по коду устройства, отзыв токена |
| `src/player` | Потоковое воспроизведение: буфер трека в памяти (`buffer`), загрузка с HTTP Range и повторами (`download`), декодер symphonia в своём потоке (`decode`), вывод через rodio (`output`), эквалайзер: фильтры и пресеты (`eq`), поддельный поток для проверок (`fake`) |
| `src/plugins` | Встроенные плагины: список FckCensor и выбор звука (`fckcensor`), метки Slopless (`slopless`), формат копирования (`copy_name`), время прослушивания и серия дней (`listen_time`: даты, периоды, огонёк, файл `listen_time.json`; учёт, таймер и экран — `src/ui/library/listen.rs`). Окно подключает их в `src/ui/library/plugins.rs` |
| `src/discord` | Discord Rich Presence: свой клиент IPC на tokio (`ipc`: кадры и поиск канала), фоновая задача с переподключением (`task`), сборка статуса и настройки (`mod`) |
| `src/sync` | «Слушать вместе»: кадры и шифрованная связь (`wire`, `crypto`, `handshake`), сообщения и проверки трека (`proto`), оценка часов (`clock`), чистая логика состояния и подстройки (`state`), секреты сопряжения в хранилище паролей (`store`), mDNS (`discovery`), служба на своём рантайме (`service`, `service/link`). Поведение в окне — `src/ui/library/together.rs` |
| `src/media_controls` | Системный плеер: чистая логика (`mod`: события → команды окна, что сообщать системе и когда; `timeline`: позиция MPRIS на момент чтения) и вызовы системы (`system`: `souvlaki` в Windows; `mpris`: своя служба MPRIS на `zbus` в Linux) |
| `src/lyrics` | Тексты песен: Яндекс и запасной LRCLib (`lrclib`), разбор LRC и поиск строки по позиции (`lrc`), кэш на диске (`cache`), выбор источника (`mod`) |
| `src/ui/mini.rs`, `src/ui/library/mini.rs` | Мини-плеер: положение, раскладка, параметры окна и цвет фона (`mini`), вид окна (`library/mini`). Окон два, поэтому приложение запускается через `iced::daemon` |
| `src/download` | Скачивание в файл: загрузка с повторами и запись через `.part` (`mod`), имя файла (`name`), очередь «по два» (`queue`), FLAC из mp4 (`flac`), теги (`tags`). Окно — `src/ui/library/downloads.rs` |
| `src/storage` | Каталоги, `config.toml`, токен в хранилище паролей |
| `src/instance.rs`, `src/tray.rs`, `src/autostart.rs` | Один экземпляр (блокировка файла и канал «показать»), значок в трее с меню, автозапуск вместе с системой |
| `src/ui/settings` | Экран «Настройки»: каркас, навигация и поиск (`mod`), элементы управления (`controls`), страницы (`pages`), страница эквалайзера и её логика (`eq_page`), виджет полос (`eq_bands`) |
| `src/ui` | Окно на iced: экраны входа и подписки (`gate`), главное окно и полноэкранный плеер (`library`), страницы и поиск (`library/pages`, `library/browse`), очередь (`queue`), обложки и размытый фон (`covers`), анимации (`anim`), спектр (`visualizer`), текст песни (`library/lyrics`, прокрутка в `lyrics_view`), тема, иконки, общие виджеты |
| `docs/api` | Спецификации API: эндпоинты, поля ответов, протокол «Слушать вместе» |
| `assets/fonts` | Шрифт Inter (SIL OFL 1.1), урезан до нужных символов (`assets/fonts/SUBSET.md`), встроен в бинарник |
| `assets/icons` | Иконки Lucide (ISC, `LICENSE-lucide.txt`) и свои в том же стиле, встроены в бинарник |
| `assets/app` | Иконка приложения: исходник SVG, PNG для окна и Linux, ICO для exe, скрипт `make_icon.py` |
| `build.rs` | Вшивает иконку и сведения о версии в `Plastinka.exe` (Windows) |
| `android` | Клиент для Android: данные и API (`data`), сервис воспроизведения, эквалайзер, нормализация и спектр (`playback`), экраны на Compose (`ui`) |
| `packaging` | Ярлык `.desktop` и `install.sh` для Linux-архива, `Info.plist` для `Plastinka.app` в macOS |
| `.github/workflows` | Сборки по push в `main` (`build-windows.yml`, `build-linux.yml`, `build-macos.yml`, `build-android.yml`) и выпуск релиза по тегу (`release.yml`, см. [RELEASING.md](RELEASING.md)) |


## Об учётных данных OAuth

API музыки принимает только токены, выданные приложению самой Яндекс Музыки.
Поэтому вход идёт от имени её Android-приложения, с его публичными client_id и
client_secret, как у всех неофициальных клиентов. Это может противоречить
условиям Яндекса.
