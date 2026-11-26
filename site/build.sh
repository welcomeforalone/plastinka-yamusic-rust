#!/bin/sh
# Сборка сайта для GitHub Pages: страницы из site/, нужные скриншоты из
# docs/screenshots, иконка из assets/app. Подставляет адрес сайта, версию
# последнего релиза и дату.
#
# Запуск: sh site/build.sh КАТАЛОГ АДРЕС_САЙТА ВЕРСИЯ
# Пример: sh site/build.sh _site https://welcomeforalone.github.io/plastinka-yamusic-rust 1.0.0
set -eu

if [ $# -ne 3 ]; then
  echo "Запуск: sh site/build.sh КАТАЛОГ АДРЕС_САЙТА ВЕРСИЯ" >&2
  exit 2
fi

out=$1
site_url=${2%/}
version=$3
root=$(cd "$(dirname "$0")/.." && pwd)
lastmod=$(date -u +%Y-%m-%d)

rm -rf "$out"
mkdir -p "$out/img"
cp -R "$root/site/." "$out/"
rm -f "$out/build.sh"
cp "$root/assets/app/plastinka.svg" "$out/favicon.svg"
cp "$root/assets/app/plastinka.png" "$out/apple-touch-icon.png"
cp "$root/docs/screenshots/social-preview.png" "$out/img/"

# На сайт уходят только скриншоты, на которые ссылаются страницы. Нет файла —
# cp падает, и сборка останавливается.
for f in $(grep -oh 'img/[a-z0-9-]*\.webp' "$out/index.html" "$out/en/index.html" | sort -u); do
  cp "$root/docs/screenshots/${f#img/}" "$out/$f"
done

find "$out" -type f \( -name '*.html' -o -name '*.xml' -o -name '*.txt' \) -exec sed -i \
  -e "s|@SITE_URL@|$site_url|g" \
  -e "s|@VERSION@|$version|g" \
  -e "s|@LASTMOD@|$lastmod|g" {} +

if grep -rlI '@[A-Z_][A-Z_]*@' "$out"; then
  echo "В файлах выше остались незаменённые метки" >&2
  exit 1
fi
