#!/bin/sh
# Установка Plastinka для текущего пользователя: без sudo, в ~/.local.
# Удаление: ./install.sh --uninstall
set -eu

dir=$(cd "$(dirname "$0")" && pwd)
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
icons="$data/icons/hicolor"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/plastinka" "$apps/plastinka.desktop" "$icons/256x256/apps/plastinka.png" "$icons/scalable/apps/plastinka.svg"
    echo "Plastinka удалена. Настройки и кэш: ~/.config/plastinka, ~/.cache/plastinka"
    exit 0
fi

mkdir -p "$bin" "$apps" "$icons/256x256/apps" "$icons/scalable/apps"
install -m 755 "$dir/plastinka" "$bin/plastinka"
install -m 644 "$dir/plastinka.png" "$icons/256x256/apps/plastinka.png"
install -m 644 "$dir/plastinka.svg" "$icons/scalable/apps/plastinka.svg"
sed "s|^Exec=.*|Exec=$bin/plastinka|" "$dir/plastinka.desktop" > "$apps/plastinka.desktop"
chmod 644 "$apps/plastinka.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" || true

echo "Установлено: $bin/plastinka"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Каталог $bin не в PATH: запускайте из меню приложений или по полному пути." ;;
esac
