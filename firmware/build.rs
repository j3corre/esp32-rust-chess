//! Build script – runs on the PC before the firmware is compiled.
//!
//! Adds the linker script `linkall.x` (from esp-hal), which describes the chip's memory
//! layout, and prints a helpful hint when linking fails.
//! Taken from the `esp-generate` template (the official esp-rs project generator).

fn main() {
    linker_be_nice();
    git_version();
    // linkall.x must be the last linker script.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

/// Version for the web and the display: `FW_GIT` = output of `git describe --always --dirty`
/// ("unknown" without git).
fn git_version() {
    let describe = std::process::Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=FW_GIT={describe}");
    // rerun when the commit / branch changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
}

/// On an error the linker calls this program again (see `--error-handling-script`)
/// and we print a hint about what is wrong based on the missing symbol.
fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                what if what.starts_with("esp_rtos_") => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-radio` has no scheduler – you need to call `esp_rtos::start`."
                    );
                    eprintln!();
                }
                "free"
                | "malloc"
                | "calloc"
                | "get_free_internal_heap_size"
                | "malloc_internal"
                | "realloc_internal"
                | "calloc_internal"
                | "free_internal" => {
                    eprintln!();
                    eprintln!("💡 Is the `esp-alloc` dependency missing?");
                    eprintln!();
                }
                _ => (),
            },
            _ => {
                std::process::exit(1);
            }
        }

        std::process::exit(0);
    }

    // RISC-V links with rust-lld (argument passed directly), Xtensa (ESP32-S3) with gcc (argument
    // for ld via -Wl,).
    let prefix = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("xtensa") {
        "-Wl,"
    } else {
        ""
    };
    println!(
        "cargo:rustc-link-arg={prefix}--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
