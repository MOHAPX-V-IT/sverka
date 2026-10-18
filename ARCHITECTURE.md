# Архитектура

clap CLI → валидированный каталог → WalkDir без follow_links → bounded scoped workers → SHA-256 → BTreeMap schema 1 → atomic output. Проверка использует тот же scanner без checkpoint cache; diff — чистое сравнение метаданных/хешей с консервативным обнаружением перемещений.

Checkpoint append-only JSONL: header фиксирует root/excludes/schema, каждая запись — завершённый файл. Пишущий mutex сериализует строки и sync_data. Сбой не публикует частичный финальный snapshot. Resume использует size+mtime как cache validator; verify перепроверяет реальные байты. Это компромисс локального архивного инструмента; неподвижность корня обеспечивается оператором.

Ошибки чтения, invalid schema, permission denied и несовместимый checkpoint завершаются exit 2, без создания частичного output. Записи детерминированно отсортированы; только timestamp создания различается. Публичные функции hash_file/create_snapshot/load/diff доступны из lib для интеграции.
