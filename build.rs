//! Вшивает в Plastinka.exe иконку и сведения о программе (свойства файла).

use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    println_rerun("assets/app/plastinka.ico");
    // Хост сборки может отличаться от цели: смотрим на цель.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return ExitCode::SUCCESS;
    }
    windows_resources()
}

fn println_rerun(path: &str) {
    let _ = writeln!(io::stdout(), "cargo::rerun-if-changed={path}");
}

#[cfg(windows)]
fn windows_resources() -> ExitCode {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app/plastinka.ico")
        .set("ProductName", "Plastinka")
        .set("FileDescription", "Plastinka")
        .set("OriginalFilename", "Plastinka.exe")
        .set("InternalName", "Plastinka.exe")
        .set(
            "LegalCopyright",
            "Неофициальный клиент, не связан с ООО «Яндекс»",
        );
    match res.compile() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let _ = writeln!(
                io::stderr(),
                "не удалось вшить иконку в exe (нужен rc.exe из Windows SDK): {err}"
            );
            ExitCode::FAILURE
        }
    }
}

/// Сборка под Windows с другого хоста: ресурсы не вшиваем.
#[cfg(not(windows))]
fn windows_resources() -> ExitCode {
    let _ = writeln!(
        io::stdout(),
        "cargo::warning=иконка exe не вшита: сборка под Windows не на Windows"
    );
    ExitCode::SUCCESS
}
