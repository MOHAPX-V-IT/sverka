# Проверка 0.1.0-rc.1

2026-10-18, Windows x64, Rust 1.99.1 stable GNU, portable MinGW GCC 16.2.0.

* `cargo test --locked`: PASS, 2 unit + 4 integration tests. SHA-256 known vector, path validation, snapshot/resume, exclusions, move/change/missing, сохранность старого output, повреждённый хвост checkpoint, защита обычных файлов, CLI JSON/exit codes/help.
* `cargo clippy --all-targets -- -D warnings`: PASS.
* `cargo fmt --check`: PASS.
* `cargo build --release --locked`: PASS.
* Реальный release exe: snapshot src → target/qa.snapshot.json с checkpoint, затем verify --json: integrity_ok=true, unchanged=2.
* Финальная повторная проверка текущего кода: fmt --check, test --locked, clippy --all-targets --locked -- -D warnings, build --release --locked — exit 0.

Сборка macOS/Linux не выполнялась. Полная point-in-time согласованность изменяемого каталога не заявляется; resume доверяет size/mtime, verify хеширует заново. Все source-file операции read-only; запись направлена в явно указанные snapshot/checkpoint paths.

Добавлена конфигурация GitHub Actions `.github/workflows/checks.yml`: Ubuntu Rust stable, fmt/test/clippy/release build с --locked для Cargo-команд. YAML проверен локальным парсером; workflow на GitHub ещё не запускался.
