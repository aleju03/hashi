// Builds lazer-writer/ with the .NET SDK for Hashi to carry. Without an SDK the file is empty and
// saving to lazer skins is off; HASHI_LAZER_WRITER=required makes that an error, for releases.

use std::path::{Path, PathBuf};
use std::process::Command;

fn dotnet() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "dotnet.exe" } else { "dotnet" };
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join(exe));
    let roots = ["DOTNET_ROOT", "HOME", "USERPROFILE"].into_iter().filter_map(|k| std::env::var_os(k).map(PathBuf::from)).flat_map(|d| [d.join(exe), d.join(".dotnet").join(exe)]);
    on_path.chain(roots).find(|p| p.is_file())
}

fn build(project: &Path, out: &Path) -> Option<PathBuf> {
    let dotnet = dotnet()?;
    let artifacts = out.join("lazer-writer");
    let ok = Command::new(&dotnet)
        .args(["build", "-c", "Release", "-nologo", "--artifacts-path"])
        .arg(&artifacts)
        .arg(project.join("LazerWriter.csproj"))
        .env("DOTNET_ROOT", dotnet.parent()?)
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_NOLOGO", "1")
        .status()
        .ok()?
        .success();
    let dll = artifacts.join("bin").join("LazerWriter").join("release").join("LazerWriter.dll");
    (ok && dll.is_file()).then_some(dll)
}

fn main() {
    let project = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("..").join("lazer-writer");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for f in ["Program.cs", "LazerWriter.csproj"] {
        println!("cargo:rerun-if-changed={}", project.join(f).display());
    }
    println!("cargo:rerun-if-env-changed=HASHI_LAZER_WRITER");
    let dest = out.join("LazerWriter.dll");
    match build(&project, &out) {
        Some(dll) => {
            std::fs::copy(dll, &dest).unwrap();
        }
        None if std::env::var("HASHI_LAZER_WRITER").as_deref() == Ok("required") => panic!("lazer-writer needs the .NET SDK to build"),
        None => {
            println!("cargo:warning=no .NET SDK, this build can't save to lazer skins");
            std::fs::write(&dest, b"").unwrap();
        }
    }
}
