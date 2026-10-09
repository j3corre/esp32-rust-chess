//! Prints the firmware (ELF32) section sizes – how much flash and static RAM it takes –
//! and checks the size of the main-thread stack.
//!
//! ```text
//! cargo run -p elf-size -- firmware/target/riscv32imac-unknown-none-elf/release/esp32-chess
//! ```

use std::fs;

fn u16le(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn u32le(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// Verified minimum main-thread stack (with Wi-Fi) – see the check at the end of the report.
const MIN_MAIN_STACK: usize = 16 * 1024;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: elf-size <path to ELF file>");
    let elf = fs::read(&path).expect("cannot read the ELF file");
    assert_eq!(&elf[..4], b"\x7fELF", "not an ELF file");
    assert_eq!(elf[4], 1, "expected a 32-bit ELF");
    let shoff = u32le(&elf, 0x20);
    let shentsize = u16le(&elf, 0x2E);
    let shnum = u16le(&elf, 0x30);
    let shstrndx = u16le(&elf, 0x32);
    let strtab = u32le(&elf, shoff + shstrndx * shentsize + 0x10);

    let (mut flash, mut ram) = (0usize, 0usize);
    println!("{:<28} {:>10} {:>10}  kind", "section", "address", "size");
    let mut main_stack = None;
    for i in 0..shnum {
        let h = shoff + i * shentsize;
        let name_off = u32le(&elf, h);
        let kind = u32le(&elf, h + 4);
        let flags = u32le(&elf, h + 8);
        let addr = u32le(&elf, h + 0x0C);
        let size = u32le(&elf, h + 0x14);
        let name: String = elf[strtab + name_off..]
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as char)
            .collect();
        // Only sections loaded into memory (SHF_ALLOC).
        // "*_dummy" = mirror of another section on the second bus (C3/S3) – do not count twice.
        if flags & 2 == 0 || size == 0 || name.ends_with("_dummy") {
            continue;
        }
        // RAM: ESP32-C6 0x4080_0000..; ESP32-C3/S3 DRAM 0x3FC8_0000.. and IRAM 0x4037_0000..
        // (two buses of the same SRAM – the sections do not overlap, the sum is correct).
        // Everything else = flash.
        let in_ram = (0x4080_0000..0x4088_0000).contains(&addr)
            || (0x3FC8_0000..0x3FD0_0000).contains(&addr)
            || (0x4037_0000..0x403E_0000).contains(&addr);
        let nobits = kind == 8;
        let label = match (in_ram, nobits) {
            (true, true) => "RAM (zeroed)",
            (true, false) => "RAM + flash",
            _ => "flash",
        };
        if in_ram {
            ram += size;
        }
        if !nobits {
            flash += size;
        }
        println!("{name:<28} {addr:>#10x} {size:>10}  {label}");
        if name == ".stack" {
            main_stack = Some(size);
        }
    }
    println!("\nFlash total:  {:>7} B ({} kB)", flash, flash / 1024);
    println!("Static RAM:   {:>7} B ({} kB)", ram, ram / 1024);
    // Main-thread stack = the rest of the SRAM. The linker only enforces 8 kB, but Wi-Fi
    // initialisation (esp-radio creates tasks) needs more – with ~10 kB the firmware crashes at
    // startup.
    if let Some(stack) = main_stack {
        println!("Main-thread stack: {stack} B");
        if stack < MIN_MAIN_STACK {
            println!(
                "!!! WARNING: the main-thread stack is smaller than the verified {MIN_MAIN_STACK} B – \
                 the firmware may crash when Wi-Fi starts (panic \"main stack's guard\")."
            );
        }
    }
}
