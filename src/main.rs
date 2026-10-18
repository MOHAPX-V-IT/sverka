use clap::{Parser, Subcommand};
use std::{path::PathBuf, process::ExitCode};
use sverka::{create_snapshot, diff, load};
#[derive(Parser)]
#[command(
    version,
    about = "Сверка: SHA-256 каталоги, сравнение и проверка целостности",
    long_about = "Создаёт стабильные SHA-256 снимки. Никогда не изменяет исходные файлы. Symlinks пропускаются. Коды: 0 — успех/без различий, 1 — различия, 2 — ошибка."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long, global = true, help = "Машиночитаемый JSON в stdout")]
    json: bool,
}
#[derive(Subcommand)]
enum Command {
    #[command(about = "Создать или возобновить снимок каталога")]
    Snapshot {
        root: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short, long, default_value_t = 4)]
        workers: usize,
        #[arg(long, help = "Исключить относительный путь/каталог; можно повторять")]
        exclude: Vec<String>,
        #[arg(long, help = "JSONL checkpoint для прерванной работы")]
        checkpoint: Option<PathBuf>,
        #[arg(
            long,
            help = "Переиспользовать checkpoint при совпадении size/mtime (после снимка запустите verify)"
        )]
        resume: bool,
    },
    #[command(
        about = "Сравнить два снимка; уникальные совпадения hash/size распознаются как перемещения"
    )]
    Diff { old: PathBuf, new: PathBuf },
    #[command(about = "Заново хешировать каталог и найти изменения, пропажи и повреждения")]
    Verify {
        snapshot: PathBuf,
        #[arg(long, help = "Проверить другую копию каталога")]
        root: Option<PathBuf>,
        #[arg(short, long, default_value_t = 4)]
        workers: usize,
    },
}
fn run(cli: Cli) -> Result<u8, String> {
    match cli.command {
        Command::Snapshot {
            root,
            output,
            workers,
            exclude,
            checkpoint,
            resume,
        } => {
            let s = create_snapshot(
                &root,
                Some(&output),
                checkpoint.as_deref(),
                resume,
                workers,
                exclude,
            )?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"status":"saved","output":output,"files":s.files.len(),"schema":s.schema})
                );
            } else {
                println!(
                    "Снимок сохранён: {}\nФайлов: {} · SHA-256 · schema 1",
                    output.display(),
                    s.files.len()
                );
            }
            Ok(0)
        }
        Command::Diff { old, new } => {
            let a = load(&old)?;
            let b = load(&new)?;
            let changes = diff(&a, &b);
            print_changes(&changes, cli.json, false);
            Ok(u8::from(changes.has_changes()))
        }
        Command::Verify {
            snapshot,
            root,
            workers,
        } => {
            let old = load(&snapshot)?;
            let root = root.unwrap_or_else(|| PathBuf::from(&old.root));
            let current = create_snapshot(&root, None, None, false, workers, old.excludes.clone())?;
            let changes = diff(&old, &current);
            print_changes(&changes, cli.json, true);
            Ok(u8::from(changes.has_changes()))
        }
    }
}
fn print_changes(c: &sverka::Changes, json: bool, verify: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({"mode":if verify{"verify"}else{"diff"},"integrity_ok":!c.has_changes(),"changes":c})
        );
    } else {
        println!("Без изменений: {}", c.unchanged);
        for p in &c.added {
            println!("+ добавлен {p}");
        }
        for p in &c.missing {
            println!("- отсутствует {p}");
        }
        for p in &c.changed {
            println!("! содержимое изменено / возможное повреждение {p}");
        }
        for m in &c.moved {
            println!("> перемещён {} → {}", m.from, m.to);
        }
        if !c.has_changes() {
            println!("Целостность подтверждена.");
        }
    }
}
fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            if json {
                eprintln!("{}", serde_json::json!({"error":e}));
            } else {
                eprintln!("Ошибка: {e}");
            }
            ExitCode::from(2)
        }
    }
}
