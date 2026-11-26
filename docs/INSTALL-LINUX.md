# Установка в Linux подробно

Сборка одна на все дистрибутивы: она собрана с glibc 2.35 и работает на
Arch, CachyOS, Manjaro, Fedora 36+, openSUSE, Ubuntu 22.04+, Debian 12+ и других.
Из системных библиотек нужна только ALSA (`libasound.so.2`, на большинстве
систем уже стоит; через PipeWire и PulseAudio звук тоже идёт), остальное
окно подгружает само: Wayland или X11, Vulkan или OpenGL.

Установка и обновление одной последовательностью. Команды по одной в строке,
работают и в bash, и в fish. Архив каждый раз скачивается по прямой ссылке
на последний релиз в отдельный каталог и заменяет старый файл с тем же именем:

```sh
mkdir -p ~/plastinka-install
cd ~/plastinka-install
rm -rf plastinka-linux-x86_64 plastinka-linux-x86_64.tar.gz
curl -fLO https://github.com/welcomeforalone/plastinka-yamusic-rust/releases/latest/download/plastinka-linux-x86_64.tar.gz
tar -xzf plastinka-linux-x86_64.tar.gz
cd plastinka-linux-x86_64
sh install.sh
```

`sh install.sh` — надёжный вариант, он не зависит от шебанга. Обычный
запуск — `./install.sh`. Программа ставится в `~/.local/bin/Plastinka`;
старую `~/.local/bin/plastinka` от прежних версий установка удаляет.
Запустить без установки: `./Plastinka` в распакованной папке. Удаление —
`sh install.sh --uninstall` (убирает оба имени).

Какая версия стоит: `Plastinka --version` (или
`~/.local/bin/Plastinka --version`, если `~/.local/bin` не в PATH).

Проверить архив (файл `.sha256` тоже берём по прямой ссылке, в том же
каталоге до распаковки):

```sh
curl -fLO https://github.com/welcomeforalone/plastinka-yamusic-rust/releases/latest/download/plastinka-linux-x86_64.tar.gz.sha256
sha256sum -c plastinka-linux-x86_64.tar.gz.sha256
```

Если `./install.sh` не запускается с ошибкой про окончания строк Windows
(`\r\n`, «The file uses Windows line endings», «bad interpreter»), значит,
распакован архив прежней версии: у всех релизов одно имя файла, а браузер
сохраняет новый как `plastinka-linux-x86_64(1).tar.gz`, и `tar` берёт старый.
Лучше скачать заново командами выше. Быстрое исправление в распакованной
папке:

```sh
sed -i 's/\r$//' install.sh plastinka.desktop
sh install.sh
```

Пакеты, если чего-то нет:

```sh
# Arch, CachyOS, Manjaro
sudo pacman -S --needed alsa-lib libxkbcommon vulkan-icd-loader
# Debian, Ubuntu
sudo apt install libasound2 libxkbcommon0 libvulkan1
# Fedora
sudo dnf install alsa-lib libxkbcommon vulkan-loader
```

Токен хранится в Secret Service: нужен gnome-keyring, KWallet (с включённым
Secret Service) или KeePassXC. В KDE Plasma (CachyOS по умолчанию) KWallet
обычно уже работает.
