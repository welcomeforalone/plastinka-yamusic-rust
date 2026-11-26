## Что сделано

<!-- Коротко: что меняется и зачем. Если закрывает issue — «Closes #…» -->

## Как проверить

<!-- Шаги, на какой системе проверяли -->

## Чеклист

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` и `cargo test` проходят
- [ ] для Android: `./gradlew :app:testDebugUnitTest` в `android/`
- [ ] нет `unwrap`, `expect`, `panic!` и индексации в рабочем коде
- [ ] токен не попадает в файлы, логи и тексты ошибок
- [ ] новые зависимости — с совместимой лицензией (указана в описании)
- [ ] документация в `docs/` обновлена, если поведение изменилось
- [ ] если PR сделан с ИИ-агентом: агент читал [AGENTS.md](https://github.com/welcomeforalone/plastinka-yamusic-rust/blob/main/AGENTS.md), я сам просмотрел весь diff и проверил изменение руками (укажите инструмент)
